//! Simple two-sided maker that continuously cancels and reposts quotes over WebSocket.
//!
//! Behaviour:
//! - Subscribes to order-book + market stats + account orders via WebSocket.
//! - On every new order-book snapshot, cancels existing quotes and reposts immediately.
//! - Target spread = fee-aware floor above the live spread.
//! - Quotes are symmetric around a clamped mark (falls back to mid if mark missing).
//! - All transactions are sent as a single WebSocket batch (cancel + 2x create).
//!
//! Environment variables (same as other SDK examples):
//! - `LIGHTER_PRIVATE_KEY`
//! - `LIGHTER_ACCOUNT_INDEX` (or `ACCOUNT_INDEX`)
//! - `LIGHTER_API_KEY_INDEX`
//! - Optional: `LIGHTER_API_URL`, `LIGHTER_WS_URL`

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    tx_executor::{send_batch_tx_ws, TX_TYPE_CANCEL_ALL_ORDERS, TX_TYPE_CREATE_ORDER},
    types::{AccountId, ApiKeyIndex, BaseQty, MarketId, Nonce, Price},
    ws_client::WsEvent,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::time::Duration;
use tokio::time::{interval, Instant};

const MARKET_ID: i32 = 1; // e.g., BTC-PERP on your exchange
const ORDER_SIZE: f64 = 0.0002; // 20 lots if size_decimals = 4
const MAX_IDLE_BEFORE_REFRESH: Duration = Duration::from_millis(120);
const FORCE_REFRESH_INTERVAL: Duration = Duration::from_millis(400);
const BASE_SPREAD_PCT: f64 = 0.000025;
const MARK_FRESH_FOR: Duration = Duration::from_millis(200);
const MAKER_FEE_PCT: f64 = 0.00002; // 0.002%
const TARGET_EDGE_BPS: f64 = 0.8; // configurable risk edge in bps
const MIN_DWELL: Duration = Duration::from_millis(120);
const MIN_HOP_TICKS: i64 = 1; // set to 2 for fewer updates
const MAX_NO_MESSAGE_TIME: Duration = Duration::from_secs(15); // Reconnect if NO messages for 15s
const MAX_CONNECTION_AGE: Duration = Duration::from_secs(300); // Force reconnect every 5 minutes

#[derive(Debug, Deserialize)]
struct AccountOrderView {
    #[serde(default)]
    is_ask: Option<bool>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    market_index: Option<u32>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "remaining_base_amount")]
    remaining_base_amount: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    banner();

    // --- Environment/config ---
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

    // --- Client + signer ---
    let client = LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key)
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .account_index(AccountId::new(account_index))
        .build()
        .await
        .context("Failed to build LighterClient")?;

    let signer = client
        .signer()
        .context("Client not configured with signer (private key missing)?")?;

    log_action("Connected REST + signer ready");

    // --- Market metadata (price/size decimals) ---
    let metadata = client
        .orders()
        .book_details(Some(MarketId::new(MARKET_ID)))
        .await
        .context("Failed to fetch market metadata")?;
    let detail = metadata
        .order_book_details
        .into_iter()
        .find(|d| d.market_id == MARKET_ID)
        .context("Market metadata missing for requested market")?;

    let price_multiplier: i64 = 10_i64
        .checked_pow(detail.price_decimals as u32)
        .context("price_decimals overflow")?;
    let size_multiplier: i64 = 10_i64
        .checked_pow(detail.size_decimals as u32)
        .context("size_decimals overflow")?;
    let tick_size = 1.0 / price_multiplier as f64;
    let base_qty = BaseQty::try_from((ORDER_SIZE * size_multiplier as f64).round() as i64)
        .map_err(|e| anyhow!("Invalid base quantity: {e}"))?;

    // --- WebSocket subscriptions ---
    let auth_token = client.create_auth_token(None)?;
    let mut stream = client
        .ws()
        .subscribe_order_book(MarketId::new(MARKET_ID))
        .subscribe_market_stats(MarketId::new(MARKET_ID))
        .subscribe_account_all_orders(AccountId::new(account_index))
        .connect()
        .await
        .context("Failed to establish WebSocket stream")?;
    stream.connection_mut().set_auth_token(auth_token.clone());

    log_action(
        "WebSocket subscriptions established (order_book + market_stats + account_all_orders)",
    );

    // Reconnection loop
    let mut reconnect_count = 0;
    'reconnect: loop {
        if reconnect_count > 0 {
            log_action(&format!("Attempting reconnection #{}", reconnect_count));
            // Re-establish WebSocket connection
            match client
                .ws()
                .subscribe_order_book(MarketId::new(MARKET_ID))
                .subscribe_market_stats(MarketId::new(MARKET_ID))
                .subscribe_account_all_orders(AccountId::new(account_index))
                .connect()
                .await
            {
                Ok(new_stream) => {
                    stream = new_stream;
                    stream.connection_mut().set_auth_token(auth_token.clone());
                    log_action("WebSocket reconnected successfully");
                }
                Err(e) => {
                    log_action(&format!("Failed to reconnect: {}. Retrying in 5s...", e));
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    reconnect_count += 1;
                    continue 'reconnect;
                }
            }
        }

        // --- Runtime state ---
        let connection_start = Instant::now();
        let mut last_refresh = Instant::now() - MAX_IDLE_BEFORE_REFRESH;
        let mut last_orderbook_update = Instant::now();
        let mut last_any_message = Instant::now(); // Track ANY WebSocket message
        let mut last_mark: Option<(f64, Instant)> = None;
        let mut last_targets: Option<(i64, i64)> = None;
        let mut last_targets_change = Instant::now();
        let mut po_cancels: VecDeque<Instant> = VecDeque::new();
        let mut has_active_bid = false;
        let mut has_active_ask = false;

        let mut watchdog = interval(Duration::from_secs(5));

        loop {
            tokio::select! {
                    _ = watchdog.tick() => {
                        let now = Instant::now();

                        // Check for complete WebSocket silence (no messages at all)
                        if now.duration_since(last_any_message) > MAX_NO_MESSAGE_TIME {
                            log_action(&format!(
                                "⚠️  No WebSocket messages for {:?} — reconnecting",
                                now.duration_since(last_any_message)
                            ));
                            reconnect_count += 1;
                            break; // Break inner loop to reconnect
                        }

                        // Check connection age
                        if now.duration_since(connection_start) > MAX_CONNECTION_AGE {
                            log_action("WebSocket connection aged out — forcing reconnection");
                            reconnect_count += 1;
                            break; // Break inner loop to reconnect
                        }

                        if last_orderbook_update.elapsed() > Duration::from_secs(30) {
                            log_action("⚠️  No orderbook updates for >30s — waiting for new data");
                        }
                    }
                    maybe_event = stream.next() => {
                    let Some(next) = maybe_event else {
                        log_action("WebSocket stream ended");
                        break;
                    };

                    match next {
                        Ok(WsEvent::OrderBook(ob)) => {
                            let now = Instant::now();
                            last_orderbook_update = now;
                            last_any_message = now; // Update message timestamp

                            if ob.state.bids.is_empty() || ob.state.asks.is_empty() {
                                continue;
                            }

                            // Skip zero-size orders (cancelled/filled)
                            let (top_bid_str, raw_best_bid) = match ob.state.bids.iter().find_map(|level| {
                                let size_str = level.remaining_base_amount.as_deref().unwrap_or(&level.size);
                                if size_str == "0" || size_str == "0.0" || size_str == "0.00" || size_str.is_empty() {
                                    return None;
                                }
                                let size: f64 = size_str.parse().ok()?;
                                if size > 0.0 {
                                    let price = parse_price(&level.price).ok()?;
                                    Some((level.price.clone(), price))
                                } else {
                                    None
                                }
                            }) {
                                Some(result) => result,
                                None => continue,
                            };
                            let (top_ask_str, raw_best_ask) = match ob.state.asks.iter().find_map(|level| {
                                let size_str = level.remaining_base_amount.as_deref().unwrap_or(&level.size);
                                if size_str == "0" || size_str == "0.0" || size_str == "0.00" || size_str.is_empty() {
                                    return None;
                                }
                                let size: f64 = size_str.parse().ok()?;
                                if size > 0.0 {
                                    let price = parse_price(&level.price).ok()?;
                                    Some((level.price.clone(), price))
                                } else {
                                    None
                                }
                            }) {
                                Some(result) => result,
                                None => continue,
                            };
                            log_action(&format!(
                                "raw top-of-book strings bid={} ask={} | parsed bid={:.4} ask={:.4}",
                                top_bid_str, top_ask_str, raw_best_bid, raw_best_ask
                            ));

                            if raw_best_ask <= raw_best_bid {
                                log_action(&format!(
                                    "⚠️  Invalid order book snapshot (ask {:.4} <= bid {:.4}); ignoring update",
                                    raw_best_ask, raw_best_bid
                                ));
                                continue;
                            }

                            let need_force_refresh = last_refresh.elapsed() >= FORCE_REFRESH_INTERVAL;
                            let mid_raw = 0.5 * (raw_best_bid + raw_best_ask);
                            let mark_price = match last_mark {
                                Some((mark, ts)) if ts.elapsed() <= MARK_FRESH_FOR => mark,
                                _ => mid_raw,
                            };
                            let max_drift = 0.25 * (raw_best_ask - raw_best_bid);
                            let center_price = mark_price
                                .clamp(mid_raw - max_drift, mid_raw + max_drift)
                                .max(tick_size);

                            let target_edge_pct = TARGET_EDGE_BPS * 0.0001;
                            let spread_pct =
                                (2.0 * MAKER_FEE_PCT + target_edge_pct).max(BASE_SPREAD_PCT);

                            let to_ticks_floor = |px: f64| ((px / tick_size).floor()) as i64;
                            let to_ticks_ceil = |px: f64| ((px / tick_size).ceil()) as i64;
                            let from_ticks = |ticks: i64| ticks as f64 * tick_size;

                            let mut center_ticks = to_ticks_floor(center_price);
                            let half_ticks =
                                (((center_price * spread_pct * 0.5) / tick_size).round() as i64).max(1);

                            let inventory_position_ticks = 0_i64;
                            center_ticks = center_ticks.saturating_add(inventory_position_ticks);

                            let raw_bid_t = center_ticks - half_ticks;
                            let raw_ask_t = center_ticks + half_ticks;
                            let mut bid_t = raw_bid_t;
                            let mut ask_t = raw_ask_t;

                            let best_bid_t = to_ticks_floor(raw_best_bid);
                            let best_ask_t = to_ticks_ceil(raw_best_ask);
                            if best_ask_t - best_bid_t < 1 {
                                continue;
                            }

                            let now = Instant::now();
                            po_cancels.retain(|ts| now.duration_since(*ts) <= Duration::from_millis(1500));
                            let po_slack = if po_cancels.len() >= 2 { 2 } else { 1 };

                            let max_bid_t = best_ask_t - po_slack;
                            let min_ask_t = best_bid_t + po_slack;

                            bid_t = bid_t.min(max_bid_t);
                            ask_t = ask_t.max(min_ask_t);

                            if ask_t - bid_t < 2 {
                                let mid_t = (bid_t + ask_t) / 2;
                                bid_t = mid_t - 1;
                                ask_t = mid_t + 1;
                            }

                            if ask_t <= bid_t {
                                continue;
                            }

                            let hop_ok = last_targets
                                .map(|(prev_bid, prev_ask)| {
                                    (bid_t - prev_bid).abs() >= MIN_HOP_TICKS
                                        || (ask_t - prev_ask).abs() >= MIN_HOP_TICKS
                                })
                                .unwrap_or(true);

                            let missing_side = !has_active_bid || !has_active_ask;

                            if !hop_ok && !need_force_refresh && !missing_side {
                                continue;
                            }

                            let targets_changed = last_targets
                                .map(|(prev_bid, prev_ask)| prev_bid != bid_t || prev_ask != ask_t)
                                .unwrap_or(true);

                            if targets_changed {
                                if last_targets_change.elapsed() < MIN_DWELL
                                    && !need_force_refresh
                                    && !missing_side
                                {
                                    continue;
                                }
                            } else if !need_force_refresh && !missing_side {
                                continue;
                            }

                            if last_refresh.elapsed() < MAX_IDLE_BEFORE_REFRESH
                                && !need_force_refresh
                                && !missing_side
                            {
                                continue;
                            }

                            const BASE_GUARD_TICKS: i64 = 30;
                            let guard_slack = half_ticks.max(BASE_GUARD_TICKS);
                            if bid_t < best_bid_t - guard_slack || ask_t > best_ask_t + guard_slack {
                                log_action(&format!(
                                    "guard: targets too far from raw BBO; skipping refresh (bid_t={}, best_bid_t={}, ask_t={}, best_ask_t={}, guard_slack={})",
                                    bid_t, best_bid_t, ask_t, best_ask_t, guard_slack
                                ));
                                continue;
                            }

                            if targets_changed {
                                last_targets_change = now;
                            }
                            last_targets = Some((bid_t, ask_t));

                            let bid_price = from_ticks(bid_t);
                            let ask_price = from_ticks(ask_t);
                            if bid_price <= 0.0 {
                                log_action("guard: computed bid <= 0; skipping refresh");
                                continue;
                            }

                            let drift_ticks = ((center_price - mid_raw) / tick_size).round() as i64;

                            log_action(&format!(
                                "mid_raw={:.4} mark={:.4} target_spread={:.4} drift={:+}t | raw=({},{}) eff=({},{}) slack={} (reason=orderbook)",
                                mid_raw,
                                mark_price,
                                center_price * spread_pct,
                                drift_ticks,
                                raw_bid_t,
                                raw_ask_t,
                                bid_t,
                                ask_t,
                                po_slack
                            ));

                            let bid_ticks = Price::ticks(bid_t);
                            let ask_ticks = Price::ticks(ask_t);

                            // ---- Sign transactions ----
                            let (cancel_api_key, cancel_nonce) = signer.next_nonce().await?;
                            let cancel_payload = signer
                                .sign_cancel_all_orders(0, 0, Some(cancel_nonce), Some(cancel_api_key))
                                .await
                                .context("Failed to sign cancel-all")?;

                            let (bid_api_key, bid_nonce) = signer.next_nonce().await?;
                            let bid_signed = client
                                .order(MarketId::new(MARKET_ID))
                                .buy()
                                .qty(base_qty.clone())
                                .limit(bid_ticks)
                                .post_only()
                                .with_api_key(ApiKeyIndex::new(bid_api_key))
                                .with_nonce(Nonce::new(bid_nonce))
                                .sign()
                                .await
                                .context("Failed to sign bid order")?;

                            let (ask_api_key, ask_nonce) = signer.next_nonce().await?;
                            let ask_signed = client
                                .order(MarketId::new(MARKET_ID))
                                .sell()
                                .qty(base_qty.clone())
                                .limit(ask_ticks)
                                .post_only()
                                .with_api_key(ApiKeyIndex::new(ask_api_key))
                                .with_nonce(Nonce::new(ask_nonce))
                                .sign()
                                .await
                                .context("Failed to sign ask order")?;

                            let mut batch: Vec<(u8, String)> = Vec::with_capacity(3);
                            batch.push((TX_TYPE_CANCEL_ALL_ORDERS, cancel_payload));
                            batch.push((TX_TYPE_CREATE_ORDER, bid_signed.payload().to_string()));
                            batch.push((TX_TYPE_CREATE_ORDER, ask_signed.payload().to_string()));

                            let send_start = Instant::now();
                            let responses = send_batch_tx_ws(stream.connection_mut(), batch.clone())
                                .await
                                .context("Batch submission failed")?;

                            let cancel_ok = responses.get(0).copied().unwrap_or(false);
                            let bid_ok = responses.get(1).copied().unwrap_or(false);
                            let ask_ok = responses.get(2).copied().unwrap_or(false);

                            if cancel_ok && bid_ok && ask_ok {
                                has_active_bid = true;
                                has_active_ask = true;
                                log_action(&format!(
                                    "Refreshed quotes: bid {:.4} / ask {:.4} (spread {:.4}, {:?})",
                                    bid_price,
                                    ask_price,
                                    ask_price - bid_price,
                                    send_start.elapsed()
                                ));
                            } else {
                                log_action(&format!(
                                    "⚠️  Batch rejected (cancel_ok={}, bid_ok={}, ask_ok={})",
                                    cancel_ok, bid_ok, ask_ok
                                ));
                                for (idx, (tx_type, payload)) in batch.iter().enumerate() {
                                    let label = match idx {
                                        0 => "cancel_all",
                                        1 => "bid",
                                        2 => "ask",
                                        _ => "unknown",
                                    };
                                    log_action(&format!(
                                        "  └─ [{}] type={} payload={}",
                                        label,
                                        tx_type,
                                        truncate(payload, 120)
                                    ));
                                }
                                if !bid_ok {
                                    has_active_bid = false;
                                }
                                if !ask_ok {
                                    has_active_ask = false;
                                }
                                if !bid_ok || !ask_ok {
                                    po_cancels.push_back(Instant::now());
                                }
                            }

                            last_refresh = Instant::now();
                        }
                        Ok(WsEvent::Account(envelope)) => {
                            last_any_message = Instant::now(); // Update message timestamp
                            if let Some(orders_obj) =
                                envelope.event.as_value().get("orders").and_then(|v| v.as_object())
                            {
                                let mut bid_found = false;
                                let mut ask_found = false;
                                let mut observed = false;

                                for (market_key, orders_val) in orders_obj {
                                    let parsed_market = match market_key.parse::<i32>() {
                                        Ok(id) => id,
                                        Err(_) => continue,
                                    };

                                    if parsed_market != MARKET_ID {
                                        continue;
                                    }

                                    observed = true;

                                    match orders_val {
                                        Value::Array(orders) => {
                                            if orders.is_empty() {
                                                bid_found = false;
                                                ask_found = false;
                                            }
                                            for order_val in orders {
                                                if let Ok(view) = serde_json::from_value::<AccountOrderView>(
                                                    order_val.clone(),
                                                ) {
                                                    if let Some(market_index) = view.market_index {
                                                        if market_index != MARKET_ID as u32 {
                                                            continue;
                                                        }
                                                    }
                                                    let is_active = view
                                                        .remaining_base_amount
                                                        .as_ref()
                                                        .and_then(|s| s.parse::<f64>().ok())
                                                        .map(|qty| qty > 0.0)
                                                        .unwrap_or_else(|| {
                                                            matches!(
                                                                view.status
                                                                    .as_deref()
                                                                    .map(|s| s.to_ascii_lowercase()),
                                                                Some(ref st) if st == "open"
                                                                    || st == "resting"
                                                                    || st == "working"
                                                                    || st == "pending"
                                                            )
                                                        });
                                                    if !is_active {
                                                        continue;
                                                    }
                                                    let is_ask = view.is_ask.unwrap_or_else(|| {
                                                        matches!(
                                                            view.side.as_deref(),
                                                            Some("sell")
                                                                | Some("ask")
                                                                | Some("SELL")
                                                                | Some("ASK")
                                                        )
                                                    });
                                                    if is_ask {
                                                        ask_found = true;
                                                    } else {
                                                        bid_found = true;
                                                    }
                                                }
                                            }
                                        }
                                        Value::Null => {
                                            bid_found = false;
                                            ask_found = false;
                                        }
                                        _ => {}
                                    }
                                }

                                if observed {
                                    has_active_bid = bid_found;
                                    has_active_ask = ask_found;
                                } else if envelope.snapshot {
                                    has_active_bid = false;
                                    has_active_ask = false;
                                }
                            }
                        }
                        Ok(WsEvent::Unknown(raw)) => {
                            last_any_message = Instant::now(); // Update message timestamp
                            if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                                if let Some(code) = value.get("code") {
                                    log_action(&format!("WS status: {} raw={}", code, truncate(&raw, 200)));
                                } else if let Some(obj) = value.get("error") {
                                    let code = obj
                                        .get("code")
                                        .and_then(|v| v.as_i64())
                                        .unwrap_or_default();
                                    if code == 30003 {
                                        // already subscribed — safe to ignore after reconnect
                                        continue;
                                    }
                                    log_action(&format!("WS error: {} raw={}", obj, truncate(&raw, 200)));
                                }
                            }
                        }
                        Ok(WsEvent::MarketStats(stats)) => {
                            last_any_message = Instant::now(); // Update message timestamp
                            if let Ok(mark) = parse_price(&stats.market_stats.mark_price) {
                                last_mark = Some((mark, Instant::now()));
                            }
                        }
                        Ok(_other) => { /* ignore other events in this simple bot */ }
                        Err(err) => {
                            log_action(&format!("❌ WebSocket error: {err}"));
                            break;
                        }
                    }
                }
            }
        } // End of inner loop (will break to reconnect)
    } // End of reconnection loop (infinite - will keep reconnecting)
}

fn banner() {
    println!("══════════════════════════════════════════════════════════════");
    println!("  SIMPLE TWO-SIDED WS MAKER  (market id = {MARKET_ID})");
    println!("══════════════════════════════════════════════════════════════\n");
}

fn parse_price(text: &str) -> Result<f64> {
    text.parse::<f64>()
        .map_err(|err| anyhow!("Invalid price '{text}': {err}"))
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        format!("{}…", &value[..max])
    }
}

fn log_action(message: &str) {
    println!(
        "[{}] {}",
        Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        message
    );
}
