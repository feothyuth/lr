//! REST-driven two-sided maker: polls the public REST API for top-of-book data,
//! then submits cancel + bid/ask batches over the WebSocket transaction channel.
//! This is useful when you want to avoid depending on the order-book WebSocket
//! stream (e.g. while investigating data quality or throttling issues) but still
//! benefit from the low-latency transaction submission path.
//!
//! Environment variables:
//! - `LIGHTER_PRIVATE_KEY`
//! - `LIGHTER_ACCOUNT_INDEX` (or `ACCOUNT_INDEX`)
//! - `LIGHTER_API_KEY_INDEX`
//! - Optional: `LIGHTER_API_URL`

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    models::SimpleOrder,
    signer_client::SignerClient,
    tx_executor::{send_batch_tx_ws, TX_TYPE_CANCEL_ALL_ORDERS, TX_TYPE_CREATE_ORDER},
    types::{AccountId, ApiKeyIndex, BaseQty, MarketId, Nonce, Price},
    ws_client::WsEvent,
};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::{interval, Instant, MissedTickBehavior};

const MARKET_ID: i32 = 1;
const ORDER_SIZE: f64 = 0.0002;
const BASE_SPREAD_PCT: f64 = 0.00002;
const MAKER_FEE_PCT: f64 = 0.00002;
const TARGET_EDGE_BPS: f64 = 0.8;
const REST_POLL_INTERVAL: Duration = Duration::from_millis(150);
const BOOK_LIMIT: i64 = 50;
const MIN_REFRESH_INTERVAL: Duration = Duration::from_millis(80);
const MAX_NO_MESSAGE_TIME: Duration = Duration::from_secs(12);
const MAX_CONNECTION_AGE: Duration = Duration::from_secs(300);
const NONCE_REPEAT_THRESHOLD: Duration = Duration::from_millis(300);

#[tokio::main]
async fn main() -> Result<()> {
    banner();

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

    let signer = client
        .signer()
        .context("Client not configured with signer (private key missing)?")?;

    log_action("Connected REST + signer ready");

    let details = client
        .orders()
        .book_details(Some(MarketId::new(MARKET_ID)))
        .await
        .context("Failed to load market metadata")?
        .order_book_details
        .into_iter()
        .find(|d| d.market_id == MARKET_ID)
        .context("Market metadata missing for requested market")?;

    let price_multiplier = 10_i64
        .checked_pow(details.price_decimals as u32)
        .context("price_decimals overflow")?;
    let size_multiplier = 10_i64
        .checked_pow(details.size_decimals as u32)
        .context("size_decimals overflow")?;
    let tick_size = 1.0 / price_multiplier as f64;
    let base_qty = BaseQty::try_from((ORDER_SIZE * size_multiplier as f64).round() as i64)
        .map_err(|err| anyhow!("Invalid base quantity: {err}"))?;

    let auth_token = client.create_auth_token(None)?;

    let mut stream = connect_tx_stream(&client, &auth_token, account_index).await?;

    let mut reconnects = 0usize;
    loop {
        if reconnects > 0 {
            log_action(&format!("Reconnecting (attempt #{reconnects})"));
            stream = connect_tx_stream(&client, &auth_token, account_index).await?;
        }

        let mut rest_timer = interval(REST_POLL_INTERVAL);
        rest_timer.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let connection_start = Instant::now();
        let mut last_any_message = Instant::now();
        let mut last_refresh = Instant::now() - MIN_REFRESH_INTERVAL;
        let mut last_cancel_api_key: Option<i32> = None;
        let mut last_bid_api_key: Option<i32> = None;
        let mut last_ask_api_key: Option<i32> = None;
        let mut nonce_refresh_times: HashMap<i32, Instant> = HashMap::new();

        loop {
            tokio::select! {
                _ = rest_timer.tick() => {
                    match fetch_best_levels(&client).await {
                        Ok(Some((best_bid, best_ask))) => {
                            last_any_message = Instant::now();
                            if best_ask <= best_bid {
                                log_action(&format!("⚠️  REST returned crossed book bid={best_bid:.4} ask={best_ask:.4}; skipping tick"));
                                continue;
                            }

                            if last_refresh.elapsed() < MIN_REFRESH_INTERVAL {
                                continue;
                            }

                            let mid = 0.5 * (best_bid + best_ask);

                            let spread_pct =
                                (2.0 * MAKER_FEE_PCT + TARGET_EDGE_BPS * 0.0001).max(BASE_SPREAD_PCT);
                            let half_width = mid * spread_pct * 0.5;

                            let mut bid_price = (mid - half_width).max(tick_size);
                            let mut ask_price = mid + half_width;

                            let bid_ticks = (bid_price / tick_size).floor() as i64;
                            let mut ask_ticks = (ask_price / tick_size).ceil() as i64;
                            if ask_ticks <= bid_ticks {
                                ask_ticks = bid_ticks + 1;
                            }
                            bid_price = bid_ticks as f64 * tick_size;
                            ask_price = ask_ticks as f64 * tick_size;

                            log_action(&format!(
                                "mid={mid:.4} -> bid={bid_price:.4} ask={ask_price:.4}"
                            ));

                            let cancel_tx = {
                                let (api_key, nonce) = signer.next_nonce().await?;
                                last_cancel_api_key = Some(api_key);
                                signer
                                    .sign_cancel_all_orders(0, 0, Some(nonce), Some(api_key))
                                    .await
                                    .context("failed to sign cancel-all")?
                            };

                            let bid_tx = {
                                let (api_key, nonce) = signer.next_nonce().await?;
                                last_bid_api_key = Some(api_key);
                                client
                                    .order(MarketId::new(MARKET_ID))
                                    .buy()
                                    .qty(base_qty.clone())
                                    .limit(Price::ticks(bid_ticks))
                                    .post_only()
                                    .with_api_key(ApiKeyIndex::new(api_key))
                                    .with_nonce(Nonce::new(nonce))
                                    .sign()
                                    .await
                                    .context("failed to sign bid order")?
                                    .payload()
                                    .to_string()
                            };

                            let ask_tx = {
                                let (api_key, nonce) = signer.next_nonce().await?;
                                last_ask_api_key = Some(api_key);
                                client
                                    .order(MarketId::new(MARKET_ID))
                                    .sell()
                                    .qty(base_qty.clone())
                                    .limit(Price::ticks(ask_ticks))
                                    .post_only()
                                    .with_api_key(ApiKeyIndex::new(api_key))
                                    .with_nonce(Nonce::new(nonce))
                                    .sign()
                                    .await
                                    .context("failed to sign ask order")?
                                    .payload()
                                    .to_string()
                            };

                            let batch = vec![
                                (TX_TYPE_CANCEL_ALL_ORDERS, cancel_tx),
                                (TX_TYPE_CREATE_ORDER, bid_tx),
                                (TX_TYPE_CREATE_ORDER, ask_tx),
                            ];

                            let start = Instant::now();
                            let results = send_batch_tx_ws(stream.connection_mut(), batch.clone())
                                .await
                                .context("batch submission failed")?;

                            log_action(&format!(
                                "Batch result cancel={} bid={} ask={} ({:?})",
                                results.get(0).copied().unwrap_or(false),
                                results.get(1).copied().unwrap_or(false),
                                results.get(2).copied().unwrap_or(false),
                                start.elapsed()
                            ));

                            last_refresh = Instant::now();
                        }
                        Ok(None) => {
                            log_action("⚠️  REST order book empty; skipping tick");
                        }
                        Err(err) => {
                            log_action(&format!("⚠️  REST fetch failed: {err}"));
                        }
                    }
                }
                maybe_event = stream.next() => {
                    let Some(event) = maybe_event else {
                        log_action("WebSocket stream closed by server");
                        reconnects += 1;
                        break;
                    };

                    match event {
                        Ok(WsEvent::Pong) => {
                            last_any_message = Instant::now();
                        }
                        Ok(WsEvent::Connected) => {
                            log_action("WebSocket (tx) connected");
                            last_any_message = Instant::now();
                        }
                        Ok(WsEvent::Unknown(raw)) => {
                            last_any_message = Instant::now();
                            if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                                if let Some(err) = value.get("error") {
                                    if err
                                        .get("code")
                                        .and_then(|c| c.as_i64())
                                        .map(|c| c == 30003)
                                        .unwrap_or(false)
                                    {
                                        if stream.connection().suppressing_already_subscribed() {
                                            continue;
                                        }
                                    }
                                    log_action(&format!("WS error: {}", err));
                                    if err
                                        .get("message")
                                        .and_then(|m| m.as_str())
                                        .map(|m| m.contains("invalid nonce"))
                                        .unwrap_or(false)
                                        || err
                                            .get("code")
                                            .and_then(|c| c.as_i64())
                                            .map(|c| c == 21104)
                                            .unwrap_or(false)
                                    {
                                        handle_nonce_failure(
                                            &signer,
                                            &mut nonce_refresh_times,
                                            &mut last_cancel_api_key,
                                            &mut last_bid_api_key,
                                            &mut last_ask_api_key,
                                        )
                                        .await;
                                        continue;
                                    }
                                } else if let Some(code) = value.get("code") {
                                    if code.as_i64() == Some(30003) {
                                        if stream.connection().suppressing_already_subscribed() {
                                            continue;
                                        }
                                    }
                                    log_action(&format!(
                                        "WS status: {} raw={}",
                                        code,
                                        truncate(&raw, 200)
                                    ));
                                    if code.as_i64() == Some(21104) {
                                        handle_nonce_failure(
                                            &signer,
                                            &mut nonce_refresh_times,
                                            &mut last_cancel_api_key,
                                            &mut last_bid_api_key,
                                            &mut last_ask_api_key,
                                        )
                                        .await;
                                        continue;
                                    }
                                }
                            }
                        }
                        Ok(_) => {
                            last_any_message = Instant::now();
                        }
                        Err(err) => {
                            log_action(&format!("❌ WebSocket error: {err}"));
                            reconnects += 1;
                            break;
                        }
                    }
                }
            }

            if Instant::now().duration_since(last_any_message) > MAX_NO_MESSAGE_TIME {
                log_action("⚠️  No REST/WS activity; reconnecting WebSocket");
                reconnects += 1;
                break;
            }

            if Instant::now().duration_since(connection_start) > MAX_CONNECTION_AGE {
                log_action("WebSocket connection aged out; reconnecting");
                reconnects += 1;
                break;
            }
        }
    }
}

fn parse_price(text: &str) -> Result<f64> {
    text.parse::<f64>()
        .map_err(|err| anyhow!("Invalid price '{text}': {err}"))
}

fn parse_amount(text: &str) -> Result<f64> {
    text.parse::<f64>()
        .map_err(|err| anyhow!("Invalid amount '{text}': {err}"))
}

fn level_active(order: &SimpleOrder) -> bool {
    parse_amount(&order.remaining_base_amount)
        .map(|v| v > 1e-12)
        .unwrap_or(false)
}

async fn fetch_best_levels(client: &LighterClient) -> Result<Option<(f64, f64)>> {
    let book = client
        .orders()
        .book(MarketId::new(MARKET_ID), BOOK_LIMIT)
        .await
        .context("failed to fetch order book")?;

    if book.code != 200 {
        return Err(anyhow!(
            "REST order book returned code {} message {:?}",
            book.code,
            book.message
        ));
    }

    let best_bid = book
        .bids
        .iter()
        .filter(|order| level_active(order))
        .filter_map(|order| parse_price(&order.price).ok())
        .fold(None::<f64>, |acc, price| match acc {
            Some(best) => Some(best.max(price)),
            None => Some(price),
        });

    let best_ask = book
        .asks
        .iter()
        .filter(|order| level_active(order))
        .filter_map(|order| parse_price(&order.price).ok())
        .fold(None::<f64>, |acc, price| match acc {
            Some(best) => Some(best.min(price)),
            None => Some(price),
        });

    Ok(match (best_bid, best_ask) {
        (Some(bid), Some(ask)) => Some((bid, ask)),
        _ => None,
    })
}

async fn handle_nonce_failure(
    signer: &SignerClient,
    nonce_refresh_times: &mut HashMap<i32, Instant>,
    last_cancel_api_key: &mut Option<i32>,
    last_bid_api_key: &mut Option<i32>,
    last_ask_api_key: &mut Option<i32>,
) {
    let now = Instant::now();
    for api in [*last_cancel_api_key, *last_bid_api_key, *last_ask_api_key]
        .into_iter()
        .flatten()
    {
        if let Some(prev) = nonce_refresh_times.get(&api) {
            if now.duration_since(*prev) <= NONCE_REPEAT_THRESHOLD {
                if let Err(err) = signer.acknowledge_nonce_failure(api).await {
                    log_action(&format!(
                        "⚠️  Failed to acknowledge nonce failure for api_key {}: {err}",
                        api
                    ));
                } else {
                    log_action(&format!(
                        "⚠️  Acknowledged repeated nonce failure for api_key {}",
                        api
                    ));
                }
            }
        }
        match signer.refresh_nonce(api).await {
            Ok(()) => {
                nonce_refresh_times.insert(api, now);
            }
            Err(refresh_err) => {
                log_action(&format!(
                    "⚠️  Failed to refresh nonce for api_key {}: {refresh_err}",
                    api
                ));
            }
        }
    }
    *last_cancel_api_key = None;
    *last_bid_api_key = None;
    *last_ask_api_key = None;
}

async fn connect_tx_stream(
    client: &LighterClient,
    auth_token: &str,
    account_index: i64,
) -> Result<lighter_client::ws_client::WsStream> {
    let mut ws = client
        .ws()
        .subscribe_transactions()
        .subscribe_account_all_orders(AccountId::new(account_index))
        .connect()
        .await
        .context("Failed to establish WebSocket stream")?;
    ws.connection_mut().set_auth_token(auth_token.to_string());
    log_action("WebSocket (tx) connected & authenticated");
    Ok(ws)
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        format!("{}…", &value[..max])
    }
}

fn banner() {
    println!("══════════════════════════════════════════════════════════════");
    println!("  SIMPLE TWO-SIDED MM (REST driven quotes, WS tx)");
    println!("══════════════════════════════════════════════════════════════\n");
}

fn log_action(message: &str) {
    println!(
        "[{}] {}",
        Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        message
    );
}
