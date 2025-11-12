//! Avellaneda-Stoikov market maker powered by the lighter_client SDK.
//!
//! This example connects to the Lighter exchange, streams market data and
//! account updates, and manages a two-sided quoting book using the core
//! Avellaneda strategy implemented under `src/avellaneda/`.

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use lighter_client::{
    avellaneda::{AvellanedaConfig, AvellanedaStrategy},
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, BaseQty, MarketId},
    ws_client::{
        AccountEvent, AccountEventEnvelope, OrderBookState, TransactionData, WsConnection, WsEvent,
        WsStream,
    },
};
use serde_json::Value;
use std::{
    fs::{create_dir_all, OpenOptions},
    io::{BufWriter, Write},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{interval, sleep, MissedTickBehavior},
};
use tracing::{error, info, warn};

use lighter_client::avellaneda::execution::ExecutionEngine;
use lighter_client::avellaneda::types::StrategyDecision;

#[derive(Clone)]
struct MarketView {
    book: OrderBookState,
    mark_price: Option<f64>,
    mark_timestamp: Option<Instant>,
    last_order_book_update: Instant,
}

enum ExecutionCommand {
    Decision(StrategyDecision, Instant),
    Account { snapshot: bool, event: AccountEvent },
    Transactions(Vec<TransactionData>),
    Reconnect(oneshot::Sender<anyhow::Result<bool>>),
}

struct MetricsRow {
    timestamp_ms: i128,
    net_pnl: f64,
    total_fills: u64,
    inventory_base: f64,
    inventory_norm: f64,
    reservation_price: Option<f64>,
    spread_raw_bps: Option<f64>,
    spread_effective_bps: Option<f64>,
    markout_bid_bps: Option<f64>,
    markout_ask_bps: Option<f64>,
    same_side_p95_bid: f64,
    same_side_p95_ask: f64,
    flip_rate_hz: f64,
    flips_200ms: usize,
    inventory_age_s: f64,
    pnl_per_million: Option<f64>,
    global_pause: bool,
    kill_bid: bool,
    kill_ask: bool,
}

struct MetricsCsvLogger {
    writer: BufWriter<std::fs::File>,
}

impl MetricsCsvLogger {
    fn new(path: PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        let is_new = !path.exists();
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let mut writer = BufWriter::new(file);
        if is_new {
            writer.write_all(
                b"timestamp_ms,net_pnl,total_fills,inventory_base,inventory_norm,reservation_price,spread_raw_bps,spread_effective_bps,markout_bid_bps,markout_ask_bps,same_side_p95_bid,same_side_p95_ask,flip_rate_hz,flips_200ms,inventory_age_s,pnl_per_million,global_pause,kill_bid,kill_ask\n",
            )?;
        }
        Ok(Self { writer })
    }

    fn log(&mut self, row: &MetricsRow) -> std::io::Result<()> {
        writeln!(
            self.writer,
            "{},{:.6},{},{:.6},{:.6},{},{},{},{},{},{:.6},{:.6},{:.6},{},{:.6},{},{},{},{}",
            row.timestamp_ms,
            row.net_pnl,
            row.total_fills,
            row.inventory_base,
            row.inventory_norm,
            fmt_opt(row.reservation_price),
            fmt_opt(row.spread_raw_bps),
            fmt_opt(row.spread_effective_bps),
            fmt_opt(row.markout_bid_bps),
            fmt_opt(row.markout_ask_bps),
            row.same_side_p95_bid,
            row.same_side_p95_ask,
            row.flip_rate_hz,
            row.flips_200ms,
            row.inventory_age_s,
            fmt_opt(row.pnl_per_million),
            row.global_pause as i32,
            row.kill_bid as i32,
            row.kill_ask as i32,
        )
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

fn fmt_opt(value: Option<f64>) -> String {
    value
        .map(|v| format!("{:.6}", v))
        .unwrap_or_else(|| "".to_string())
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let config = AvellanedaConfig::from_file("config.toml")?;
    let private_key = std::env::var("LIGHTER_PRIVATE_KEY")
        .context("LIGHTER_PRIVATE_KEY environment variable missing")?;
    let account_index: i64 = std::env::var("LIGHTER_ACCOUNT_INDEX")
        .or_else(|_| std::env::var("ACCOUNT_INDEX"))
        .context("LIGHTER_ACCOUNT_INDEX or ACCOUNT_INDEX environment variable missing")?
        .parse()
        .context("Failed to parse account index")?;
    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")
        .unwrap_or_else(|_| "0".to_string())
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
            .context("Failed to build lighter client")?,
    );

    let market_id = MarketId::new(config.market_id);
    let book_details = client
        .orders()
        .book_details(Some(market_id))
        .await
        .context("Failed to load market metadata")?
        .order_book_details
        .into_iter()
        .find(|detail| detail.market_id == config.market_id)
        .context("Market metadata not found for requested market")?;

    let price_multiplier = 10_i64
        .checked_pow(book_details.price_decimals as u32)
        .context("price_decimals overflow")?;
    let size_multiplier = 10_i64
        .checked_pow(book_details.size_decimals as u32)
        .context("size_decimals overflow")?;

    let tick_size = 1.0 / price_multiplier as f64;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        info!("Starting Avellaneda session attempt {}", attempt);
        match run_session(
            Arc::clone(&client),
            config.clone(),
            market_id,
            account_index,
            tick_size,
            size_multiplier,
        )
        .await
        {
            Ok(()) => {
                info!("Avellaneda session completed successfully");
                break;
            }
            Err(err) => {
                error!("Avellaneda session attempt {} failed: {}", attempt, err);
                let backoff = Duration::from_secs((attempt.min(5) * 5) as u64);
                warn!("Restarting session after {:?}", backoff);
                sleep(backoff).await;
            }
        }
    }

    Ok(())
}

async fn run_session(
    client: Arc<LighterClient>,
    config: AvellanedaConfig,
    market_id: MarketId,
    account_index: i64,
    tick_size: f64,
    size_multiplier: i64,
) -> Result<()> {
    let base_qty = BaseQty::try_from((config.order_size * size_multiplier as f64).round() as i64)
        .map_err(|e| anyhow!("{}", e))
        .context("order_size rounds to zero base quantity")?;

    let details = client.account().details().await?;
    let detailed = details
        .accounts
        .iter()
        .find(|acct| acct.account_index == account_index)
        .context("Requested account not found in account details response")?;
    let account_id = AccountId::new(account_index);

    let quote_balance = parse_decimal(&detailed.available_balance).unwrap_or_default();
    let total_asset_value = parse_decimal(&detailed.total_asset_value).unwrap_or(quote_balance);
    let base_balance = extract_position_from_details(detailed, config.market_id);

    info!(
        "Initial balances | base={:.6} | quote={:.2}",
        base_balance, quote_balance
    );

    let auth_token = client
        .create_auth_token(None)
        .context("Failed to create auth token")?;
    let market_loop_token = auth_token.clone();
    let trade_loop_token = auth_token.clone();

    let mut market_ws = client
        .ws()
        .subscribe_order_book(market_id)
        .subscribe_market_stats(market_id)
        .connect()
        .await
        .context("Failed to connect market websocket")?;
    market_ws
        .connection_mut()
        .set_auth_token(auth_token.clone());

    let account_id = AccountId::new(account_index);
    let mut trade_ws = client
        .ws()
        .subscribe_account_all_orders(account_id)
        .subscribe_account_all_positions(account_id)
        .connect()
        .await
        .context("Failed to connect trading websocket")?;
    trade_ws.connection_mut().set_auth_token(auth_token.clone());

    let tx_connection = connect_transactions_stream(&client, &auth_token).await?;

    let strategy = AvellanedaStrategy::new(config.clone());
    let execution = ExecutionEngine::new(
        Arc::clone(&client),
        market_id,
        base_qty,
        tick_size,
        config.dry_run,
        config.refresh_interval_ms,
        config.fast_execution,
        config.optimistic_batch_ack,
        tx_connection,
        config.refresh_tolerance_ticks,
        Some(auth_token.clone()),
    );

    let (exec_tx, exec_rx) = mpsc::channel::<ExecutionCommand>(64);

    let (tx_snapshot, rx_snapshot) = watch::channel::<Option<MarketView>>(None);

    let market_task = tokio::spawn(market_loop(
        market_ws,
        market_id,
        tx_snapshot,
        market_loop_token.clone(),
    ));
    let execution_task = tokio::spawn(execution_loop(execution, exec_rx));

    let trade_task = tokio::spawn(trade_loop(
        trade_ws,
        rx_snapshot,
        strategy,
        exec_tx,
        base_balance,
        quote_balance,
        total_asset_value,
        tick_size,
        trade_loop_token,
    ));

    let (market_res, execution_res, trade_res) =
        tokio::try_join!(market_task, execution_task, trade_task)?;
    market_res?;
    execution_res?;
    trade_res?;

    Ok(())
}

async fn market_loop(
    mut ws: WsStream,
    market_id: MarketId,
    tx_snapshot: watch::Sender<Option<MarketView>>,
    auth_token: String,
) -> Result<()> {
    info!("Market loop started for market {}", market_id.0);
    let mut last_book: Option<OrderBookState> = None;
    let mut last_book_ts: Option<Instant> = None;
    let mut last_mark: Option<(f64, Instant)> = None;
    let mut last_market_message = Instant::now();
    let mut watchdog = interval(Duration::from_millis(500));
    watchdog.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let market_idle_timeout = Duration::from_secs(5);

    loop {
        tokio::select! {
            _ = watchdog.tick() => {
                let now = Instant::now();
                if now.duration_since(last_market_message) > market_idle_timeout {
                    warn!(
                        "Market websocket idle for >{:?}; attempting reconnect",
                        market_idle_timeout
                    );
                    if let Err(err) = ws.connection_mut().reconnect(Some(3)).await {
                        error!("Market websocket reconnect failed: {}", err);
                        return Err(err.into());
                    }
                    ws.connection_mut().set_auth_token(auth_token.clone());
                    last_market_message = Instant::now();
                    if let (Some(book), Some(ts)) = (last_book.clone(), last_book_ts) {
                        let snapshot = MarketView {
                            book,
                            mark_price: last_mark.map(|(value, _)| value),
                            mark_timestamp: last_mark.map(|(_, ts)| ts),
                            last_order_book_update: ts,
                        };
                        let _ = tx_snapshot.send(Some(snapshot));
                    }
                }
            }
            event = ws.next() => {
                match event {
                    Some(Ok(WsEvent::OrderBook(ob))) if ob.market == market_id => {
                        last_market_message = Instant::now();
                        last_book_ts = Some(last_market_message);
                        last_book = Some(ob.state.clone());
                        let snapshot = MarketView {
                            book: ob.state,
                            mark_price: last_mark.map(|(value, _)| value),
                            mark_timestamp: last_mark.map(|(_, ts)| ts),
                            last_order_book_update: last_book_ts.unwrap(),
                        };
                        let _ = tx_snapshot.send(Some(snapshot));
                    }
                    Some(Ok(WsEvent::MarketStats(stats))) => {
                        if stats.market_stats.market_id == market_id.0 as u32 {
                            last_market_message = Instant::now();
                            if let Some(mark) = parse_decimal(&stats.market_stats.mark_price) {
                                last_mark = Some((mark, Instant::now()));
                                if let (Some(book), Some(ts)) = (last_book.clone(), last_book_ts) {
                                    let snapshot = MarketView {
                                        book,
                                        mark_price: Some(mark),
                                        mark_timestamp: Some(Instant::now()),
                                        last_order_book_update: ts,
                                    };
                                    let _ = tx_snapshot.send(Some(snapshot));
                                }
                            }
                        }
                    }
                    Some(Ok(WsEvent::Pong)) | Some(Ok(WsEvent::Connected)) => {
                        last_market_message = Instant::now();
                    }
                    Some(Ok(WsEvent::Closed(info))) => {
                        warn!("Market websocket closed: {:?}", info);
                        if let Err(err) = ws.connection_mut().reconnect(Some(3)).await {
                            error!("Market websocket reconnect failed: {}", err);
                            return Err(err.into());
                        }
                        ws.connection_mut().set_auth_token(auth_token.clone());
                        last_market_message = Instant::now();
                    }
                    Some(Ok(WsEvent::Unknown(raw))) => {
                        last_market_message = Instant::now();
                        if !raw.contains("ping") && !raw.contains("pong") {
                            warn!("Unknown market WS event: {raw}");
                        }
                    }
                    Some(Ok(other)) => {
                        last_market_message = Instant::now();
                        warn!("Untracked market WS event: {:?}", other);
                    }
                    Some(Err(err)) => {
                        error!("Market websocket error: {err}");
                        if let Err(re_err) = ws.connection_mut().reconnect(Some(3)).await {
                            error!("Market websocket reconnect failed: {}", re_err);
                            return Err(re_err.into());
                        }
                        ws.connection_mut().set_auth_token(auth_token.clone());
                        last_market_message = Instant::now();
                    }
                    None => {
                        warn!("Market websocket stream ended; attempting reconnect");
                        if let Err(err) = ws.connection_mut().reconnect(Some(3)).await {
                            error!("Market websocket reconnect failed: {}", err);
                            return Err(err.into());
                        }
                        ws.connection_mut().set_auth_token(auth_token.clone());
                        last_market_message = Instant::now();
                    }
                }
            }
        }
    }
}

async fn connect_transactions_stream(
    client: &LighterClient,
    auth_token: &str,
) -> Result<WsConnection> {
    let mut stream = client
        .ws()
        .subscribe_transactions()
        .connect()
        .await
        .context("Failed to connect transaction websocket")?;
    stream
        .connection_mut()
        .set_auth_token(auth_token.to_string());
    sleep(Duration::from_millis(50)).await;
    info!("Transaction socket connected");
    Ok(stream.into_connection())
}

async fn trade_loop(
    mut ws: WsStream,
    mut rx_snapshot: watch::Receiver<Option<MarketView>>,
    mut strategy: AvellanedaStrategy,
    execution_tx: mpsc::Sender<ExecutionCommand>,
    mut base_balance: f64,
    mut quote_balance: f64,
    mut total_asset_value: f64,
    tick_size: f64,
    auth_token: String,
) -> Result<()> {
    info!("Trade loop started");

    let session_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let metrics_path = PathBuf::from(format!("logs/metrics/session-{session_ts}.csv"));
    let mut metrics_logger =
        MetricsCsvLogger::new(metrics_path).context("failed to initialise metrics logger")?;

    let mut _latest_view: Option<MarketView> = None;
    let mut latest_mid: Option<f64> = None;
    let mut status_ticker = interval(Duration::from_secs(60));
    status_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut tx_watchdog = interval(Duration::from_millis(500));
    tx_watchdog.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let tx_idle_timeout = Duration::from_secs(15);
    let mut last_tx_activity = Instant::now();
    let mut account_watchdog = interval(Duration::from_millis(500));
    account_watchdog.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let account_idle_timeout = Duration::from_secs(15);
    let mut last_account_activity = Instant::now();

    // Rate limiter for event-driven quoting (prevents spam during volatile periods)
    let min_refresh_interval = Duration::from_millis(strategy.config.refresh_interval_ms.max(20));
    let mut last_quote_attempt = Instant::now();

    loop {
        tokio::select! {
            _ = tx_watchdog.tick() => {
                let now = Instant::now();
                if now.duration_since(last_tx_activity) > tx_idle_timeout {
                    warn!("Transaction WebSocket idle for >{:?}; attempting reconnect", tx_idle_timeout);
                    let (resp_tx, resp_rx) = oneshot::channel();
                    if execution_tx
                        .send(ExecutionCommand::Reconnect(resp_tx))
                        .await
                        .is_err()
                    {
                        return Err(anyhow!("Execution loop terminated during reconnect request"));
                    }
                    match resp_rx.await {
                        Ok(Ok(true)) => {
                            info!("Transaction WebSocket reconnected via watchdog");
                            last_tx_activity = Instant::now();
                        }
                        Ok(Ok(false)) => {
                            return Err(anyhow!(
                                "Transaction watchdog reconnect attempt did not establish a new connection"
                            ));
                        }
                        Ok(Err(err)) => {
                            return Err(anyhow!(
                                "Transaction watchdog reconnect failed: {}",
                                err
                            ));
                        }
                        Err(_) => {
                            return Err(anyhow!(
                                "Execution loop dropped reconnect response"
                            ));
                        }
                    }
                }
            }
            _ = account_watchdog.tick() => {
                let now = Instant::now();
                if now.duration_since(last_account_activity) > account_idle_timeout {
                    warn!(
                        "Account/trade WebSocket idle for >{:?}; attempting reconnect",
                        account_idle_timeout
                    );
                    if let Err(err) = ws.connection_mut().reconnect(Some(3)).await {
                        return Err(anyhow!("Account/trade WebSocket reconnect failed: {}", err));
                    }
                    ws.connection_mut().set_auth_token(auth_token.clone());
                    last_account_activity = Instant::now();
                }
            }
            _ = status_ticker.tick() => {
                let metrics = strategy.metrics();
                let snapshot = strategy.inventory_snapshot();
                let participation = strategy.participation_metrics();
                let mark_bid = participation
                    .median_markout_bid_bps
                    .map(|v| format!("{:.3}", v))
                    .unwrap_or_else(|| "-".to_string());
                let mark_ask = participation
                    .median_markout_ask_bps
                    .map(|v| format!("{:.3}", v))
                    .unwrap_or_else(|| "-".to_string());
                let spread_raw = metrics
                    .last_raw_spread_bps
                    .map(|v| format!("{:.3}", v))
                    .unwrap_or_else(|| "-".to_string());
                let spread_eff = metrics
                    .last_effective_spread_bps
                    .map(|v| format!("{:.3}", v))
                    .unwrap_or_else(|| "-".to_string());
                info!(
                    "STATUS | PnL: ${:.2} | Fills: {} | Inventory: {:.6} BTC (q={:.3}) | Avg Buy: ${:.2} | Avg Sell: ${:.2} | Spread(raw={} bps, eff={} bps) | Markout(bid={} bps, ask={} bps) | SameSide(bid:{} ask:{} | p95 {:.1}/{:.1}) | Flips:{} (~{:.2} Hz) | InvAge:{:.1}s | PnL/$1M:{} | Kill(bid:{} ask:{} global:{})",
                    metrics.net_pnl,
                    metrics.total_fills,
                    snapshot.base_balance,
                    snapshot.normalized_inventory,
                    metrics.avg_buy_price,
                    metrics.avg_sell_price,
                    spread_raw,
                    spread_eff,
                    mark_bid,
                    mark_ask,
                    participation.same_side_fills_bid,
                    participation.same_side_fills_ask,
                    participation.same_side_fills_p95_bid,
                    participation.same_side_fills_p95_ask,
                    participation.flips_200ms,
                    participation.flip_rate_hz,
                    participation.inventory_age_s,
                    fmt_opt(participation.pnl_per_million),
                    participation.kill_active_bid,
                    participation.kill_active_ask,
                    participation.global_pause_active,
                );
                let timestamp_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i128;
                let metrics_row = MetricsRow {
                    timestamp_ms,
                    net_pnl: metrics.net_pnl,
                    total_fills: metrics.total_fills,
                    inventory_base: snapshot.base_balance,
                    inventory_norm: snapshot.normalized_inventory,
                    reservation_price: metrics.last_reservation_price,
                    spread_raw_bps: metrics.last_raw_spread_bps,
                    spread_effective_bps: metrics.last_effective_spread_bps,
                    markout_bid_bps: participation.median_markout_bid_bps,
                    markout_ask_bps: participation.median_markout_ask_bps,
                    same_side_p95_bid: participation.same_side_fills_p95_bid,
                    same_side_p95_ask: participation.same_side_fills_p95_ask,
                    flip_rate_hz: participation.flip_rate_hz,
                    flips_200ms: participation.flips_200ms,
                    inventory_age_s: participation.inventory_age_s,
                    pnl_per_million: participation.pnl_per_million,
                    global_pause: participation.global_pause_active,
                    kill_bid: participation.kill_active_bid,
                    kill_ask: participation.kill_active_ask,
                };
                metrics_logger
                    .log(&metrics_row)
                    .context("failed to write metrics row")?;
            }
            changed = rx_snapshot.changed() => {
                if changed.is_err() {
                    warn!("Market snapshot channel closed");
                    break;
                }

                // Clone view immediately to drop borrow guard (required for Send)
                let view_opt = rx_snapshot.borrow().clone();

                if let Some(view) = view_opt {
                    if let Some(mid) = book_mid(&view.book) {
                        latest_mid = Some(mid);
                    }
                    if let Some(mid) = latest_mid {
                        if total_asset_value <= 0.0 {
                            total_asset_value = quote_balance + base_balance * mid;
                        }
                        // Recompute quote balance to align with total asset estimate.
                        if total_asset_value > 0.0 {
                            quote_balance = (total_asset_value - base_balance * mid).max(0.0);
                        }
                        strategy.update_balances(base_balance, quote_balance);
                    }
                    _latest_view = Some(view.clone());

                    // EVENT-DRIVEN QUOTING: Quote immediately on order book update
                    let now = Instant::now();

                    // Rate limiter: Don't quote faster than min_refresh_interval
                    if now.duration_since(last_quote_attempt) < min_refresh_interval {
                        continue;
                    }

                    // Stale book check
                    if view.last_order_book_update.elapsed() > Duration::from_secs(5) {
                        continue;
                    }

                    last_quote_attempt = now;

                    // Generate and execute quote decision
                    let kill_switch = kill_switch_enabled();
                    let decision = if kill_switch {
                        StrategyDecision::Cancel("kill_switch")
                    } else {
                        strategy.on_order_book(&view.book, now)
                    };

                    match decision {
                        StrategyDecision::Skip(_) => {}
                        command => {
                            if execution_tx
                                .send(ExecutionCommand::Decision(command, now))
                                .await
                                .is_err()
                            {
                                return Err(anyhow!("Execution loop terminated while processing decision"));
                            }
                            strategy.record_quote();
                            last_tx_activity = Instant::now();
                        }
                    }
                }
            }
            maybe_event = ws.next() => {
                match maybe_event {
                    Some(Ok(WsEvent::Account(envelope))) => {
                        last_account_activity = Instant::now();
                        if execution_tx
                            .send(ExecutionCommand::Account {
                                snapshot: envelope.snapshot,
                                event: envelope.event.clone(),
                            })
                            .await
                            .is_err()
                        {
                            return Err(anyhow!(
                                "Execution loop terminated while processing account event"
                            ));
                        }
                        handle_account_event(
                            &mut strategy,
                            &envelope,
                            &mut base_balance,
                            &mut quote_balance,
                            latest_mid,
                        );
                        if let Some(mid) = latest_mid {
                            total_asset_value = quote_balance + base_balance * mid;
                        }
                    }
                    Some(Ok(WsEvent::Trade(_trade))) => {
                        last_account_activity = Instant::now();
                    }
                    Some(Ok(WsEvent::Transaction(batch))) => {
                        if batch.txs.iter().any(|tx| tx.status != 1) {
                            warn!("Transaction ack anomaly: {:?}", batch);
                        }
                        let txs = batch.txs.clone();
                        if execution_tx
                            .send(ExecutionCommand::Transactions(txs))
                            .await
                            .is_err()
                        {
                            return Err(anyhow!(
                                "Execution loop terminated while processing transaction event"
                            ));
                        }
                        last_tx_activity = Instant::now();
                    }
                    Some(Ok(WsEvent::Pong)) | Some(Ok(WsEvent::Connected)) => {
                        last_account_activity = Instant::now();
                    }
                    Some(Ok(WsEvent::Closed(info))) => {
                        warn!("Trading websocket closed: {:?}", info);
                        break;
                    }
                    Some(Ok(other)) => {
                        warn!("Untracked trading event: {:?}", other);
                    }
                    Some(Err(err)) => {
                        error!("Trading websocket error: {err}");
                        return Err(err.into());
                    }
                    None => {
                        warn!("Trading websocket stream ended");
                        break;
                    }
                }
            }
        }
    }

    let _ = metrics_logger.flush();
    Ok(())
}

fn handle_account_event(
    strategy: &mut AvellanedaStrategy,
    envelope: &AccountEventEnvelope,
    base_balance: &mut f64,
    quote_balance: &mut f64,
    latest_mid: Option<f64>,
) {
    let value = envelope.event.as_value();
    if let Some(positions_obj) = value.get("positions").and_then(|v| v.as_object()) {
        if let Some(position) = extract_position_from_map(positions_obj, strategy.config.market_id)
        {
            if (position - *base_balance).abs() > 1e-9 {
                info!(
                    "Inventory sync via account feed: {:.6} -> {:.6}",
                    *base_balance, position
                );
                *base_balance = position;
            }
        }
    }

    if let Some(available) = value.get("available_balance").and_then(parse_decimal_value) {
        *quote_balance = available;
    } else if let Some(balances) = value.get("balances").and_then(|v| v.as_object()) {
        if let Some(available) = balances
            .get("available_balance")
            .and_then(parse_decimal_value)
        {
            *quote_balance = available;
        }
    }

    if let Some(mid) = latest_mid {
        strategy.update_balances(*base_balance, *quote_balance);
        info!(
            "Account update | base={:.6} quote={:.2} mid={:.2}",
            base_balance, quote_balance, mid
        );
    }
}

async fn execution_loop(
    mut execution: ExecutionEngine,
    mut rx: mpsc::Receiver<ExecutionCommand>,
) -> Result<()> {
    while let Some(command) = rx.recv().await {
        match command {
            ExecutionCommand::Decision(decision, timestamp) => {
                execution.handle_decision(decision, timestamp).await?;
            }
            ExecutionCommand::Account { snapshot, event } => {
                execution.ingest_account_event(snapshot, &event).await?;
            }
            ExecutionCommand::Transactions(txs) => {
                execution.ingest_transaction_event(txs).await?;
            }
            ExecutionCommand::Reconnect(response) => {
                let result = execution.reconnect_transactions().await;
                let _ = response.send(result);
            }
        }
    }
    Ok(())
}

fn extract_position_from_details(
    detailed: &lighter_client::models::DetailedAccount,
    market_id: i32,
) -> f64 {
    detailed
        .positions
        .iter()
        .find(|pos| pos.market_id == market_id)
        .and_then(|pos| {
            let raw = parse_decimal(&pos.position)?;
            let sign = pos.sign.clamp(-1, 1);
            Some(if sign == 0 {
                raw
            } else {
                raw.abs() * sign as f64
            })
        })
        .unwrap_or(0.0)
}

fn extract_position_from_map(
    positions_obj: &serde_json::Map<String, Value>,
    market_id: i32,
) -> Option<f64> {
    let key = market_id.to_string();
    let matched = positions_obj.get(&key).or_else(|| {
        positions_obj.values().find(|val| {
            val.get("market_id")
                .and_then(|v| v.as_i64())
                .map(|id| id == market_id as i64)
                .unwrap_or(false)
        })
    })?;

    let raw = matched
        .get("position")
        .and_then(parse_decimal_value)
        .unwrap_or(0.0);
    let sign_hint = matched
        .get("sign")
        .and_then(|v| v.as_i64())
        .map(|s| s.clamp(-1, 1));
    let side_hint = matched
        .get("side")
        .and_then(|v| v.as_str())
        .map(|s| s.to_ascii_lowercase());

    let derived_sign = sign_hint.filter(|s| *s != 0).or_else(|| {
        side_hint.as_deref().map(|side| match side {
            "sell" | "short" => -1,
            "buy" | "long" => 1,
            _ => 0,
        })
    });

    Some(match derived_sign {
        Some(sign) if sign != 0 => raw.abs() * sign as f64,
        _ => raw,
    })
}

fn parse_decimal(input: &str) -> Option<f64> {
    input.replace('_', "").parse::<f64>().ok()
}

fn parse_decimal_value(value: &Value) -> Option<f64> {
    if let Some(s) = value.as_str() {
        parse_decimal(s)
    } else {
        value.as_f64()
    }
}

fn book_mid(book: &OrderBookState) -> Option<f64> {
    // Skip zero-size orders (cancelled/filled)
    let bid = book.bids.iter().find_map(|level| {
        let size_str = level.remaining_base_amount.as_deref().unwrap_or(&level.size);
        if size_str == "0" || size_str == "0.0" || size_str == "0.00" || size_str.is_empty() {
            return None;
        }
        let size: f64 = size_str.parse().ok()?;
        if size > 0.0 {
            level.price.parse::<f64>().ok()
        } else {
            None
        }
    })?;
    let ask = book.asks.iter().find_map(|level| {
        let size_str = level.remaining_base_amount.as_deref().unwrap_or(&level.size);
        if size_str == "0" || size_str == "0.0" || size_str == "0.00" || size_str.is_empty() {
            return None;
        }
        let size: f64 = size_str.parse().ok()?;
        if size > 0.0 {
            level.price.parse::<f64>().ok()
        } else {
            None
        }
    })?;
    if ask > bid {
        Some(0.5 * (bid + ask))
    } else {
        None
    }
}

fn kill_switch_enabled() -> bool {
    std::env::var("AVELLANEDA_KILL_SWITCH")
        .ok()
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn init_tracing() {
    if tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .finish(),
    )
    .is_err()
    {
        // Tracing already initialised elsewhere.
    }
}
