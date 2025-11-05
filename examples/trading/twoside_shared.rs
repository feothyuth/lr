use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    signer_client::SignerClient,
    tx_executor::{send_batch_tx_ws, TX_TYPE_CANCEL_ALL_ORDERS, TX_TYPE_CREATE_ORDER},
    types::{AccountId, ApiKeyIndex, BaseQty, MarketId, Nonce, Price},
    ws_client::{OrderBookLevel, OrderBookState, WsEvent, WsStream},
};
use tokio::{
    sync::watch,
    time::{interval, MissedTickBehavior},
};

const MAX_IDLE_BEFORE_REFRESH: Duration = Duration::from_millis(120);
const FORCE_REFRESH_INTERVAL: Duration = Duration::from_millis(400);
const PO_SLACK_WINDOW: Duration = Duration::from_millis(1500);
const MIN_HOP_TICKS: i64 = 1;
const BASE_GUARD_TICKS: i64 = 30;
const MAX_BOOK_STALE: Duration = Duration::from_secs(30); // Only consider stale after 30s of no updates
const MAX_NO_MESSAGE_TIME: Duration = Duration::from_secs(15); // Reconnect if NO messages for 15s
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(5);
const MAX_CONNECTION_AGE: Duration = Duration::from_secs(300); // Reconnect every 5 minutes
const MAKER_FEE_PCT: f64 = 0.00002;

#[derive(Clone)]
pub struct EngineConfig {
    pub market_id: i32,
    pub order_size: f64,
    pub default_half_spread_pct: f64,
    pub dry_run: bool,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct MarketView {
    pub book: OrderBookState,
    pub mark_price: Option<f64>,
    pub mark_timestamp: Option<Instant>,
    pub last_order_book_update: Instant,
}

#[allow(dead_code)]
pub struct StrategyInput<'a> {
    pub view: &'a MarketView,
    pub tick_size: f64,
    pub last_posted_prices: Option<(f64, f64)>,
    pub now: Instant,
}

pub struct StrategyOutput {
    pub center_price: f64,
    pub half_spread_pct: f64,
    pub label: &'static str,
    pub diagnostics: Vec<(&'static str, f64)>,
}

pub trait QuoteStrategy {
    fn compute(&mut self, input: StrategyInput<'_>) -> Option<StrategyOutput>;
    fn record_quote(&mut self, _bid_price: f64, _ask_price: f64) {}
}

struct TradeParams {
    market: MarketId,
    base_qty: BaseQty,
    tick_size: f64,
    default_half_spread_pct: f64,
    dry_run: bool,
    order_size: f64,
}

pub async fn run_two_sided<S>(setup: EngineConfig, mut strategy: S) -> Result<()>
where
    S: QuoteStrategy + Send + 'static,
{
    dotenvy::dotenv().ok();

    let private_key =
        std::env::var("LIGHTER_PRIVATE_KEY").context("LIGHTER_PRIVATE_KEY not set")?;
    let account_index: i64 = std::env::var("LIGHTER_ACCOUNT_INDEX")
        .or_else(|_| std::env::var("ACCOUNT_INDEX"))
        .context("LIGHTER_ACCOUNT_INDEX or ACCOUNT_INDEX not set")?
        .parse()
        .context("Failed to parse account index")?;
    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")?
        .parse()
        .context("Failed to parse LIGHTER_API_KEY_INDEX")?;
    let api_url = std::env::var("LIGHTER_API_URL")
        .unwrap_or_else(|_| "https://mainnet.zklighter.elliot.ai".to_string());

    let client = Arc::new(
        LighterClient::builder()
            .api_url(api_url)
            .private_key(private_key)
            .account_index(AccountId::new(account_index))
            .api_key_index(ApiKeyIndex::new(api_key_index))
            .build()
            .await
            .context("Failed to build LighterClient")?,
    );
    ensure_signer(&client)?;
    log_line("REST client + signer ready");

    let market = MarketId::new(setup.market_id);
    let metadata = client
        .orders()
        .book_details(Some(market))
        .await
        .context("Failed to load market metadata")?;
    let detail = metadata
        .order_book_details
        .into_iter()
        .find(|d| d.market_id == setup.market_id)
        .context("Market metadata missing for requested market")?;

    let price_multiplier = 10_i64
        .checked_pow(detail.price_decimals as u32)
        .context("price_decimals overflow")?;
    let size_multiplier = 10_i64
        .checked_pow(detail.size_decimals as u32)
        .context("size_decimals overflow")?;
    let tick_size = 1.0 / price_multiplier as f64;

    let base_qty = BaseQty::try_from((setup.order_size * size_multiplier as f64).round() as i64)
        .map_err(|err| anyhow!("Invalid base quantity: {err}"))?;

    let auth_token = client
        .create_auth_token(None)
        .context("Failed to create WebSocket auth token")?;

    let mut market_ws = client
        .ws()
        .subscribe_order_book(market)
        .subscribe_market_stats(market)
        .connect()
        .await
        .context("Failed to open market WebSocket")?;
    market_ws
        .connection_mut()
        .set_auth_token(auth_token.clone());

    let mut trade_ws = client
        .ws()
        .subscribe_transactions() // Need this to submit transactions
        .subscribe_account_all_orders(AccountId::new(account_index))
        .connect()
        .await
        .context("Failed to open trading WebSocket")?;
    trade_ws.connection_mut().set_auth_token(auth_token.clone());

    // Wait a moment for authentication to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    log_line("WebSocket connections ready (market + trading)");

    let (tx_snapshot, rx_snapshot) = watch::channel::<Option<MarketView>>(None);

    // Clone client and auth_token for the market_task
    let market_client = client.clone();
    let market_auth_token = auth_token.clone();

    let market_task = tokio::spawn(async move {
        let mut market_ws = market_ws;
        loop {
            match market_loop(market_ws, market, tx_snapshot.clone()).await {
                Err(err)
                    if err.to_string().contains("stale")
                        || err.to_string().contains("aged out") =>
                {
                    log_line(&format!("Market WebSocket needs reconnection: {err}"));
                    log_line("Reconnecting market WebSocket in 2 seconds...");
                    tokio::time::sleep(Duration::from_secs(2)).await;

                    // Reconnect WebSocket
                    match market_client
                        .ws()
                        .subscribe_order_book(market)
                        .subscribe_market_stats(market)
                        .connect()
                        .await
                    {
                        Ok(mut new_ws) => {
                            new_ws
                                .connection_mut()
                                .set_auth_token(market_auth_token.clone());
                            market_ws = new_ws;
                            log_line("Market WebSocket reconnected successfully");
                        }
                        Err(e) => {
                            log_line(&format!("Failed to reconnect market WebSocket: {e}"));
                            break;
                        }
                    }
                }
                Err(err) => {
                    log_line(&format!("Market loop error (fatal): {err:#}"));
                    break;
                }
                Ok(_) => {
                    log_line("Market loop ended normally");
                    break;
                }
            }
        }
    });

    let trade_task = tokio::spawn(async move {
        let params = TradeParams {
            market,
            base_qty,
            tick_size,
            default_half_spread_pct: setup.default_half_spread_pct,
            dry_run: setup.dry_run,
            order_size: setup.order_size,
        };
        if let Err(err) = trade_loop(client, trade_ws, rx_snapshot, &mut strategy, params).await {
            log_line(&format!("trade loop error: {err:#}"));
        }
    });

    let _ = tokio::try_join!(market_task, trade_task)?;
    Ok(())
}

async fn market_loop(
    mut ws: WsStream,
    market: MarketId,
    tx_snapshot: watch::Sender<Option<MarketView>>,
) -> Result<()> {
    log_line(&format!("Market loop started for market {}", market.0));

    let mut last_book: Option<OrderBookState> = None;
    let mut last_book_ts: Option<Instant> = None;
    let mut last_mark: Option<(f64, Instant)> = None;
    let mut last_any_message = Instant::now(); // Track ANY WebSocket message
    let connection_start = Instant::now();
    let mut watchdog = interval(WATCHDOG_INTERVAL);
    watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = watchdog.tick() => {
                let now = Instant::now();
                if now.duration_since(last_any_message) > MAX_NO_MESSAGE_TIME {
                    log_line(&format!(
                        "ERROR: No WebSocket messages for {:?}, reconnecting",
                        now.duration_since(last_any_message)
                    ));
                    return Err(anyhow::anyhow!("WebSocket silent, reconnection needed"));
                }

                if now.duration_since(connection_start) > MAX_CONNECTION_AGE {
                    log_line("WebSocket connection aged out, forcing reconnection");
                    return Err(anyhow::anyhow!("WebSocket connection aged out"));
                }
            }
            maybe_event = ws.next() => {
                let Some(event) = maybe_event else {
                    log_line("Market WebSocket stream ended");
                    break;
                };

                let now = Instant::now();
                last_any_message = now;

                match event {
                    Ok(WsEvent::OrderBook(ob)) if ob.market == market => {
                        last_book_ts = Some(now);
                        last_book = Some(ob.state.clone());

                        if let (Some(bid), Some(ask)) = (ob.state.bids.first(), ob.state.asks.first()) {
                            if let (Ok(bid_price), Ok(ask_price)) =
                                (bid.price.parse::<f64>(), ask.price.parse::<f64>())
                            {
                                log_monitor_snapshot(
                                    "INFO monitor_market",
                                    bid_price,
                                    ask_price,
                                    last_mark.map(|(value, _)| value),
                                    None,
                                    None,
                                    Some("source=market_ws"),
                                );
                            }
                        }

                        let snapshot = MarketView {
                            book: ob.state,
                            mark_price: last_mark.map(|(value, _)| value),
                            mark_timestamp: last_mark.map(|(_, ts)| ts),
                            last_order_book_update: now,
                        };
                        tx_snapshot.send(Some(snapshot)).ok();
                    }
                    Ok(WsEvent::MarketStats(stats)) => {
                        if stats.market_stats.market_id as i32 != market.0 {
                            continue;
                        }
                        match stats.market_stats.mark_price.parse::<f64>() {
                            Ok(value) => {
                                let now = Instant::now();
                                last_mark = Some((value, now));
                                if let (Some(book), Some(book_ts)) = (last_book.clone(), last_book_ts) {
                                    let snapshot = MarketView {
                                        book,
                                        mark_price: Some(value),
                                        mark_timestamp: Some(now),
                                        last_order_book_update: book_ts,
                                    };
                                    tx_snapshot.send(Some(snapshot)).ok();
                                }
                            }
                            Err(err) => log_line(&format!("malformed mark price: {err}")),
                        }
                    }
                    Ok(WsEvent::Unknown(raw)) => {
                        if raw.contains("ping") {
                            continue;
                        } else if !raw.contains("pong") {
                            log_line(&format!("Unknown WS event: {}", raw));
                        }
                    }
                    Ok(WsEvent::Closed(_)) => break,
                    Ok(_) => {}
                    Err(err) => {
                        log_line(&format!("market WS error: {err}"));
                        break;
                    }
                }
            }
        }
    }

    log_line("market loop ended");
    drop(tx_snapshot);
    Ok(())
}

async fn trade_loop<S>(
    client: Arc<LighterClient>,
    mut ws: WsStream,
    mut rx_snapshot: watch::Receiver<Option<MarketView>>,
    strategy: &mut S,
    params: TradeParams,
) -> Result<()>
where
    S: QuoteStrategy + Send + 'static,
{
    log_line(&format!(
        "Trade loop started for market {} (dry_run={})",
        params.market.0, params.dry_run
    ));
    log_line(&format!(
        "Order size: {:.6}, Tick size: {:.8}, Half spread: {:.6}",
        params.order_size, params.tick_size, params.default_half_spread_pct
    ));

    let mut last_targets: Option<(i64, i64)> = None;
    let mut last_refresh = Instant::now() - MAX_IDLE_BEFORE_REFRESH;
    let mut po_cancels: VecDeque<Instant> = VecDeque::new();

    loop {
        // Check for updates or use the current value
        tokio::select! {
            result = rx_snapshot.changed() => {
                if result.is_err() {
                    log_line("Market data channel closed, exiting trade loop");
                    break;
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                // Periodic check even without changes to handle initial state
            }
        }

        let Some(view) = rx_snapshot.borrow().clone() else {
            // No market data yet, wait a bit
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            continue;
        };

        let (raw_bid, _raw_bid_size) = top_level(&view.book.bids).context("missing bid level")?;
        let (raw_ask, _raw_ask_size) = top_level(&view.book.asks).context("missing ask level")?;
        if raw_ask <= raw_bid {
            continue;
        }
        if view.last_order_book_update.elapsed() > MAX_BOOK_STALE {
            continue;
        }

        let best_bid_t = (raw_bid / params.tick_size).floor() as i64;
        let best_ask_t = (raw_ask / params.tick_size).ceil() as i64;
        if best_ask_t - best_bid_t < 1 {
            continue;
        }

        let now = Instant::now();
        let last_prices = last_targets.map(|(bid_t, ask_t)| {
            (
                (bid_t as f64) * params.tick_size,
                (ask_t as f64) * params.tick_size,
            )
        });
        let input = StrategyInput {
            view: &view,
            tick_size: params.tick_size,
            last_posted_prices: last_prices,
            now,
        };

        let mut output = match strategy.compute(input) {
            Some(out) => out,
            None => continue,
        };

        let min_spread_pct = (2.0 * MAKER_FEE_PCT + 0.000008) * 0.5;
        output.half_spread_pct = output
            .half_spread_pct
            .max(params.default_half_spread_pct)
            .max(min_spread_pct);

        let half_price = (output.center_price * output.half_spread_pct).max(params.tick_size * 0.5);
        let guard_slack = ((half_price / params.tick_size).round() as i64).max(BASE_GUARD_TICKS);
        let mut bid_t = ((output.center_price - half_price) / params.tick_size).floor() as i64;
        let mut ask_t = ((output.center_price + half_price) / params.tick_size).ceil() as i64;

        po_cancels.retain(|ts| now.duration_since(*ts) <= PO_SLACK_WINDOW);
        let slack = if po_cancels.len() >= 2 { 2 } else { 1 };

        bid_t = bid_t.min(best_ask_t - slack);
        ask_t = ask_t.max(best_bid_t + slack);

        if ask_t - bid_t < 2 {
            let mid_t = (bid_t + ask_t) / 2;
            bid_t = mid_t - 1;
            ask_t = mid_t + 1;
        }
        if ask_t <= bid_t {
            continue;
        }

        if bid_t < best_bid_t - guard_slack || ask_t > best_ask_t + guard_slack {
            continue;
        }

        let targets_changed = last_targets
            .map(|(pb, pa)| pb != bid_t || pa != ask_t)
            .unwrap_or(true);
        let hop_ok = last_targets
            .map(|(pb, pa)| {
                (bid_t - pb).abs() >= MIN_HOP_TICKS || (ask_t - pa).abs() >= MIN_HOP_TICKS
            })
            .unwrap_or(true);
        let need_force = last_refresh.elapsed() >= FORCE_REFRESH_INTERVAL;

        if last_refresh.elapsed() < MAX_IDLE_BEFORE_REFRESH && !need_force {
            continue;
        }
        if !targets_changed && !need_force {
            continue;
        }
        if !hop_ok && !need_force {
            continue;
        }

        let bid_price = (bid_t as f64) * params.tick_size;
        let ask_price = (ask_t as f64) * params.tick_size;
        let diagnostics_joined = if output.diagnostics.is_empty() {
            None
        } else {
            Some(
                output
                    .diagnostics
                    .iter()
                    .map(|(k, v)| format!("{k}={v:.4}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        };
        let target_spread_abs = ask_price - bid_price;
        let mut extras = vec![
            format!("{} | slack={}t", output.label, slack),
            format!("half_pct={:.6}", output.half_spread_pct),
            format!("target_spread={:.6}", target_spread_abs),
        ];
        if let Some(ref joined) = diagnostics_joined {
            extras.push(joined.clone());
        }
        let extra_info = extras.join(" ");
        log_monitor_snapshot(
            "Bot",
            raw_bid,
            raw_ask,
            view.mark_price,
            Some((bid_price, params.order_size)),
            Some((ask_price, params.order_size)),
            Some(extra_info.as_str()),
        );

        if params.dry_run {
            last_targets = Some((bid_t, ask_t));
            last_refresh = Instant::now();
            strategy.record_quote(bid_price, ask_price);
            continue;
        }

        let signer = client
            .signer()
            .context("Client not configured with signer (missing private key)?")?;

        let mut batch: Vec<(u8, String)> = Vec::with_capacity(3);

        push_cancel_all(&mut batch, signer).await?;
        push_quote(
            &client,
            signer,
            params.market,
            bid_t,
            params.base_qty.clone(),
            true,
            &mut batch,
        )
        .await?;
        push_quote(
            &client,
            signer,
            params.market,
            ask_t,
            params.base_qty.clone(),
            false,
            &mut batch,
        )
        .await?;

        log_line(&format!(
            "Submitting batch with {} transactions",
            batch.len()
        ));

        let responses = send_batch_tx_ws(ws.connection_mut(), batch)
            .await
            .context("Batch submission failed")?;

        log_line(&format!("Batch response: {:?}", responses));

        if responses.iter().all(|ok| *ok) {
            last_targets = Some((bid_t, ask_t));
            last_refresh = Instant::now();
            strategy.record_quote(bid_price, ask_price);
            log_line(&format!(
                "✓ Orders placed successfully at bid={:.4} ask={:.4}",
                bid_price, ask_price
            ));
        } else {
            po_cancels.push_back(now);
            log_line(&format!("✗ Batch rejected: {responses:?}"));

            // Check which transactions failed
            for (i, ok) in responses.iter().enumerate() {
                if !ok {
                    let tx_type = if i == 0 {
                        "cancel-all"
                    } else if i == 1 {
                        "bid order"
                    } else {
                        "ask order"
                    };
                    log_line(&format!("  - Transaction {} ({}) failed", i, tx_type));
                }
            }
        }
    }
    Ok(())
}

fn log_monitor_snapshot(
    source: &str,
    raw_bid: f64,
    raw_ask: f64,
    mark: Option<f64>,
    sim_bid: Option<(f64, f64)>,
    sim_ask: Option<(f64, f64)>,
    extra: Option<&str>,
) {
    let spread = raw_ask - raw_bid;
    let ob_mid = (raw_bid + raw_ask) / 2.0;
    let mark_fmt = match mark {
        Some(value) => format!("Some({value:.6})"),
        None => "None".to_string(),
    };
    let sim_bid_fmt = format_sim(sim_bid);
    let sim_ask_fmt = format_sim(sim_ask);
    let extra_fmt = extra
        .map(|value| format!(" | {}", value))
        .unwrap_or_default();

    log_line(&format!(
        "{source}: spread {:.6} | bid {:.6} | ask {:.6} | ob_mid {:.6} | mark {mark_fmt} | sim_bid {sim_bid_fmt} | sim_ask {sim_ask_fmt}{extra_fmt}",
        spread, raw_bid, raw_ask, ob_mid
    ));
}

fn format_sim(value: Option<(f64, f64)>) -> String {
    match value {
        Some((price, size)) => format!("Some(({price:.6}, {size:.6}))"),
        None => "None".to_string(),
    }
}

async fn push_cancel_all(batch: &mut Vec<(u8, String)>, signer: &SignerClient) -> Result<()> {
    let (api_key, nonce) = signer.next_nonce().await?;
    let payload = signer
        .sign_cancel_all_orders(0, 0, Some(nonce), Some(api_key))
        .await
        .context("Failed to sign cancel-all")?;
    batch.push((TX_TYPE_CANCEL_ALL_ORDERS, payload));
    Ok(())
}

async fn push_quote(
    client: &LighterClient,
    signer: &SignerClient,
    market: MarketId,
    price_ticks: i64,
    base_qty: BaseQty,
    is_bid: bool,
    batch: &mut Vec<(u8, String)>,
) -> Result<()> {
    let (api_key, nonce) = signer.next_nonce().await?;
    let signed = if is_bid {
        client
            .order(market)
            .buy()
            .qty(base_qty)
            .limit(Price::ticks(price_ticks))
            .post_only()
            .with_api_key(ApiKeyIndex::new(api_key))
            .with_nonce(Nonce::new(nonce))
            .sign()
            .await
    } else {
        client
            .order(market)
            .sell()
            .qty(base_qty)
            .limit(Price::ticks(price_ticks))
            .post_only()
            .with_api_key(ApiKeyIndex::new(api_key))
            .with_nonce(Nonce::new(nonce))
            .sign()
            .await
    }
    .context("Failed to sign order")?;
    batch.push((TX_TYPE_CREATE_ORDER, signed.into_parts().0.tx_info));
    Ok(())
}

fn top_level(levels: &[OrderBookLevel]) -> Result<(f64, f64)> {
    let level = levels.first().context("Orderbook missing first level")?;
    let price = level
        .price
        .parse::<f64>()
        .context("Invalid price at top of book")?;
    let size = level
        .size
        .parse::<f64>()
        .or_else(|_| {
            level
                .remaining_base_amount
                .as_ref()
                .and_then(|value| value.parse::<f64>().ok())
                .ok_or_else(|| anyhow!("missing size"))
        })
        .unwrap_or(0.0);
    Ok((price, size))
}

fn ensure_signer(client: &Arc<LighterClient>) -> Result<()> {
    client
        .signer()
        .context("Client not configured with signer (missing private key)?")?;
    Ok(())
}

fn log_line(message: &str) {
    println!(
        "[{}] {}",
        Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        message
    );
}
