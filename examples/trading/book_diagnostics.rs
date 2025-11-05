//! Diagnostics tool to inspect incoming order book deltas and verify that deletes hit the
//! cached best levels. Useful for tracking persistent crossed books / stale best prices.

use anyhow::{Context, Result};
use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, MarketId},
    ws_client::{OrderBookDelta, OrderBookLevel, OrderBookState, WsEvent, WsStream},
};
use tokio::time::{interval, Instant, MissedTickBehavior};
use tracing::{error, info, warn};

const MARKET_ID: i32 = 1;
const MAX_SILENCE: std::time::Duration = std::time::Duration::from_secs(15);
const WATCHDOG: std::time::Duration = std::time::Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt().with_env_filter("info").init();

    let private_key =
        std::env::var("LIGHTER_PRIVATE_KEY").context("LIGHTER_PRIVATE_KEY not set")?;
    let account_index: i64 = std::env::var("LIGHTER_ACCOUNT_INDEX")
        .or_else(|_| std::env::var("ACCOUNT_INDEX"))
        .context("LIGHTER_ACCOUNT_INDEX or ACCOUNT_INDEX not set")?
        .parse()
        .context("Failed to parse account index")?;
    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")
        .unwrap_or_else(|_| "0".to_string())
        .parse()
        .context("Failed to parse LIGHTER_API_KEY_INDEX")?;
    let api_url = std::env::var("LIGHTER_API_URL")
        .unwrap_or_else(|_| "https://mainnet.zklighter.elliot.ai".to_string());

    let client = LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await
        .context("Failed to build LighterClient")?;

    let auth_token = client.create_auth_token(None)?;
    let mut stream = client
        .ws()
        .subscribe_order_book(MarketId::new(MARKET_ID))
        .connect()
        .await
        .context("Failed to connect WebSocket")?;
    stream.connection_mut().set_auth_token(auth_token);

    info!("Subscribed to order_book/{}", MARKET_ID);

    run_diagnostics(stream, MarketId::new(MARKET_ID)).await?;
    Ok(())
}

async fn run_diagnostics(mut stream: WsStream, market: MarketId) -> Result<()> {
    let mut last_any_message = Instant::now();
    let connection_start = Instant::now();
    let mut watchdog = interval(WATCHDOG);
    watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut saw_snapshot = false;
    let mut book: Option<OrderBookState> = None;

    loop {
        tokio::select! {
            _ = watchdog.tick() => {
                let now = Instant::now();
                if now.duration_since(last_any_message) > MAX_SILENCE {
                    warn!(
                        "No WebSocket messages for {:?}, exiting diagnostics loop",
                        now.duration_since(last_any_message)
                    );
                    break;
                }
                if now.duration_since(connection_start) > std::time::Duration::from_secs(300) {
                    warn!("Connection aged out (>5m); stopping diagnostics");
                    break;
                }
            }
            maybe_event = stream.next() => {
                let Some(event) = maybe_event else {
                    warn!("WebSocket stream ended");
                    break;
                };

                let now = Instant::now();
                last_any_message = now;

                match event {
                    Ok(WsEvent::OrderBook(ob)) if ob.market == market => {
                        if ob.delta.is_none() {
                            saw_snapshot = true;
                            book = Some(ob.state.clone());
                            info!("Received snapshot for market {}", market.0);
                            log_best(&ob.state, "snapshot");
                        } else {
                            if !saw_snapshot {
                                warn!("Delta received before snapshot for market {}", market.0);
                            }
                            if let Some(delta) = ob.delta.clone() {
                                if let Some(ref mut state) = book {
                                    log_delta_vs_best(state, &delta);
                                    apply_delta_manual(state, &delta);
                                } else if let Some(new_state) = build_state_from_delta(&delta) {
                                    warn!("No cached state yet; capturing delta as provisional book");
                                    log_delta_vs_best(&new_state, &delta);
                                    book = Some(new_state);
                                } else {
                                    warn!("Delta before snapshot with empty levels; skipping");
                                }
                            }
                            if let Some(state) = &book {
                                log_best(state, "post-delta");
                            }
                        }
                    }
                    Ok(other) => {
                        // Ignore other events but keep timestamp fresh
                        if matches!(other, WsEvent::Closed(_)) {
                            warn!("WebSocket closed by server");
                            break;
                        }
                    }
                    Err(err) => {
                        error!("WebSocket error: {err}");
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

fn log_best(book: &OrderBookState, label: &str) {
    let best_bid = book
        .bids
        .first()
        .map(|lvl| lvl.price.clone())
        .unwrap_or_else(|| "-".into());
    let best_ask = book
        .asks
        .first()
        .map(|lvl| lvl.price.clone())
        .unwrap_or_else(|| "-".into());
    info!(best_bid, best_ask, "best-of-book ({label})");
}

fn log_delta_vs_best(book: &OrderBookState, delta: &OrderBookDelta) {
    let best_bid = match book.bids.first() {
        Some(level) => level.price.clone(),
        None => {
            info!("No bids cached yet");
            return;
        }
    };
    let best_norm = normalize_price(&best_bid);

    let mut hit = false;
    let mut delete_flag = false;

    for upd in &delta.bids {
        let upd_norm = normalize_price(&upd.price);
        if upd_norm == best_norm {
            hit = true;
            let size_zero = upd.size.parse::<f64>().map(|v| v == 0.0).unwrap_or(false);
            let remaining_zero = upd
                .remaining_base_amount
                .as_deref()
                .and_then(|s| s.parse::<f64>().ok())
                .map(|v| v == 0.0)
                .unwrap_or(false);
            delete_flag = size_zero || remaining_zero;
            info!(
                current_best = best_bid,
                update_price = upd.price,
                upd_norm,
                size = upd.size,
                remaining = upd.remaining_base_amount.as_deref().unwrap_or(""),
                size_zero,
                remaining_zero,
                "Delta touched best bid"
            );
        }
    }

    if !hit {
        info!(
            current_best = best_bid,
            "Delta did NOT reference best bid; possible string mismatch"
        );
        for upd in &delta.bids {
            info!(
                update_price = upd.price,
                normalized = normalize_price(&upd.price),
                size = upd.size,
                remaining = upd.remaining_base_amount.as_deref().unwrap_or(""),
                "Bid update in delta"
            );
        }
    } else if hit && !delete_flag {
        info!(
            current_best = best_bid,
            "Delta touched best bid but did not delete it (size/remaining non-zero)"
        );
    }
}

fn normalize_price(text: &str) -> String {
    if let Ok(value) = text.parse::<f64>() {
        format!("{value:.8}")
    } else {
        text.to_string()
    }
}

fn build_state_from_delta(delta: &OrderBookDelta) -> Option<OrderBookState> {
    if delta.bids.is_empty() && delta.asks.is_empty() {
        return None;
    }
    let mut state = OrderBookState {
        bids: delta.bids.clone(),
        asks: delta.asks.clone(),
    };
    sort_levels(&mut state);
    Some(state)
}

fn apply_delta_manual(state: &mut OrderBookState, delta: &OrderBookDelta) {
    apply_side(&mut state.bids, &delta.bids, true);
    apply_side(&mut state.asks, &delta.asks, false);
    sort_levels(state);
}

fn apply_side(levels: &mut Vec<OrderBookLevel>, updates: &[OrderBookLevel], is_bid: bool) {
    for upd in updates {
        let norm = normalize_price(&upd.price);

        // deletion?
        let size_zero = upd.size.parse::<f64>().map(|v| v == 0.0).unwrap_or(false);
        let remaining_zero = upd
            .remaining_base_amount
            .as_deref()
            .and_then(|s| s.parse::<f64>().ok())
            .map(|v| v == 0.0)
            .unwrap_or(false);

        if size_zero || remaining_zero {
            if let Some(idx) = levels
                .iter()
                .position(|lvl| normalize_price(&lvl.price) == norm)
            {
                levels.remove(idx);
            }
            continue;
        }

        if let Some(idx) = levels
            .iter()
            .position(|lvl| normalize_price(&lvl.price) == norm)
        {
            levels[idx] = upd.clone();
        } else {
            levels.push(upd.clone());
        }
    }

    // limit depth growth in diagnostics
    if is_bid {
        levels.sort_by(|a, b| {
            normalize_price(&b.price)
                .partial_cmp(&normalize_price(&a.price))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    } else {
        levels.sort_by(|a, b| {
            normalize_price(&a.price)
                .partial_cmp(&normalize_price(&b.price))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    if levels.len() > 200 {
        levels.truncate(200);
    }
}

fn sort_levels(state: &mut OrderBookState) {
    state.bids.sort_by(|a, b| {
        let lhs = a.price.parse::<f64>().unwrap_or(0.0);
        let rhs = b.price.parse::<f64>().unwrap_or(0.0);
        rhs.partial_cmp(&lhs).unwrap_or(std::cmp::Ordering::Equal)
    });
    state.asks.sort_by(|a, b| {
        let lhs = a.price.parse::<f64>().unwrap_or(0.0);
        let rhs = b.price.parse::<f64>().unwrap_or(0.0);
        lhs.partial_cmp(&rhs).unwrap_or(std::cmp::Ordering::Equal)
    });
    if state.bids.len() > 200 {
        state.bids.truncate(200);
    }
    if state.asks.len() > 200 {
        state.asks.truncate(200);
    }
}
