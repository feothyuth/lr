//! Simple Order Refresher for Lighter DEX Market Making
//!
//! This example demonstrates:
//! - Placing one BID and one ASK order on the book
//! - Monitoring Order Book Mid (BBO) via WebSocket
//! - Refreshing orders when market moves >0.5 bps
//! - Maintaining continuous presence on book
//!
//! Strategy:
//! - Places limit post-only orders 0.1% away from Order Book Mid
//! - Monitors real-time Order Book updates via WebSocket
//! - Cancels and replaces orders when Mid price moves >0.5 bps
//! - Logs all actions with timestamps
//!
//! Note: Uses Order Book Mid (not Mark Price) because it's real-time.
//! Mark Price is lagged/smoothed and used for funding only.
//!
//! Run with:
//! LIGHTER_PRIVATE_KEY="..." ACCOUNT_INDEX="..." LIGHTER_API_KEY_INDEX="0" \
//! cargo run --example simple_order_refresher

use chrono::Local;
use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, BaseQty, Expiry, MarketId, Price},
    ws_client::WsEvent,
};
use time::Duration;

/// Order tracking structure
#[derive(Debug, Clone)]
struct ActiveOrders {
    bid_order_index: Option<i64>,
    ask_order_index: Option<i64>,
    initial_mid: f64,
}

impl ActiveOrders {
    fn new(initial_mid: f64) -> Self {
        Self {
            bid_order_index: None,
            ask_order_index: None,
            initial_mid,
        }
    }

    fn has_both_orders(&self) -> bool {
        self.bid_order_index.is_some() && self.ask_order_index.is_some()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   SIMPLE ORDER REFRESHER - BTC-PERP                          ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    // Load configuration
    let private_key = std::env::var("LIGHTER_PRIVATE_KEY").expect("LIGHTER_PRIVATE_KEY not set");
    let account_index: i64 = std::env::var("LIGHTER_ACCOUNT_INDEX")
        .or_else(|_| std::env::var("ACCOUNT_INDEX"))
        .expect("LIGHTER_ACCOUNT_INDEX or ACCOUNT_INDEX not set")
        .parse()?;
    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")
        .unwrap_or_else(|_| "0".to_string())
        .parse()?;

    let client = LighterClient::builder()
        .api_url("https://mainnet.zklighter.elliot.ai")
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    let market = MarketId::new(1); // BTC-PERP
    let order_size = 20; // 0.0002 BTC (minimum size)
    let spread_pct = 0.001; // 0.1% away from mid
    let refresh_threshold = 0.000002; // 0.05% (5 bps) - professional MM threshold

    log_action("Client initialized");
    println!();

    // Subscribe to Order Book + Account Orders (100% WebSocket)
    log_action("Subscribing to Order Book + Account Orders (100% WebSocket)...");
    let mut stream = client
        .ws()
        .subscribe_order_book(market)
        .subscribe_account_all_orders(AccountId::new(account_index))
        .connect()
        .await?;

    // Wait for initial order book
    let mut active_orders: Option<ActiveOrders> = None;

    log_action("Waiting for market data...");
    println!();

    // Watchdog to detect stale WebSocket (no updates for 10 seconds)
    // Note: Lighter DEX uses JSON ping/pong ({"type": "ping"} / {"type": "pong"})
    // The SDK automatically responds to server pings within ~12 seconds
    // This watchdog detects if server stops sending updates entirely
    let mut last_orderbook_update = tokio::time::Instant::now();
    let mut watchdog_interval = tokio::time::interval(tokio::time::Duration::from_secs(10));
    watchdog_interval.tick().await; // First tick completes immediately

    // Rate limit monitoring logs (only log every 5 seconds to reduce spam)
    let mut last_log_time = tokio::time::Instant::now();

    loop {
        tokio::select! {
            maybe_event = stream.next() => {
                match maybe_event {
                    Some(event) => {
                        match event? {
                            WsEvent::OrderBook(ob_event) => {
                                // Update watchdog
                                last_orderbook_update = tokio::time::Instant::now();
                // Calculate mid from Order Book BBO (real-time)
                let best_bid = ob_event.state.bids.first()
                    .and_then(|level| level.price.parse::<f64>().ok());
                let best_ask = ob_event.state.asks.first()
                    .and_then(|level| level.price.parse::<f64>().ok());

                if let (Some(bid), Some(ask)) = (best_bid, best_ask) {
                    let current_mid = (bid + ask) / 2.0;

                    // Check if we need to place or refresh orders
                    let should_refresh = match &mut active_orders {
                        None => {
                            // No orders yet - place initial orders
                            true
                        }
                        Some(orders) => {
                            // Check if orders are missing (filled/cancelled)
                            if !orders.has_both_orders() {
                                log_action("🔄 REFRESH TRIGGERED: Order filled or cancelled");
                                true
                            } else {
                                // Check if market has moved more than threshold
                                let price_change_pct = ((current_mid - orders.initial_mid).abs() / orders.initial_mid).abs();
                                let price_change_bps = price_change_pct * 10000.0;

                                // Silent monitoring - only log when refresh is triggered
                                // Heartbeat every 60 seconds to prove bot is alive
                                if last_log_time.elapsed().as_secs() >= 60 {
                                    log_action(&format!(
                                        "💓 Alive - Mid ${:.2}, drift {:.2} bps from ${:.2}",
                                        current_mid,
                                        price_change_bps,
                                        orders.initial_mid
                                    ));
                                    last_log_time = tokio::time::Instant::now();
                                }

                                if price_change_pct > refresh_threshold {
                                    log_action(&format!(
                                        "🔄 REFRESH TRIGGERED: Market moved {:.2} bps (threshold: {:.2} bps)",
                                        price_change_bps,
                                        refresh_threshold * 10000.0
                                    ));
                                    true
                                } else {
                                    false
                                }
                            }
                        }
                    };

                    if should_refresh {
                        // Cancel existing orders if any
                        if let Some(orders) = &active_orders {
                            let has_bid = orders.bid_order_index.is_some();
                            let has_ask = orders.ask_order_index.is_some();

                            if has_bid || has_ask {
                                log_action("Cancelling existing orders...");

                                // Cancel BID (if it still exists)
                                if let Some(bid_index) = orders.bid_order_index {
                                    match client.cancel(market, bid_index).submit().await {
                                        Ok(_) => log_action(&format!("  Cancelled BID (order_index: {})", bid_index)),
                                        Err(e) => log_action(&format!("  Failed to cancel BID (may be filled): {}", e)),
                                    }
                                }

                                // Cancel ASK (if it still exists)
                                if let Some(ask_index) = orders.ask_order_index {
                                    match client.cancel(market, ask_index).submit().await {
                                        Ok(_) => log_action(&format!("  Cancelled ASK (order_index: {})", ask_index)),
                                        Err(e) => log_action(&format!("  Failed to cancel ASK (may be filled): {}", e)),
                                    }
                                }

                                // Small delay to let cancellations process
                                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                            }
                        }

                        // Place new orders
                        log_action(&format!("Order Book Mid: ${:.2}", current_mid));

                        let bid_price = current_mid * (1.0 - spread_pct);
                        let ask_price = current_mid * (1.0 + spread_pct);

                        let bid_price_ticks = (bid_price * 10.0) as i64;
                        let ask_price_ticks = (ask_price * 10.0) as i64;

                        // Place BID order
                        log_action(&format!(
                            "Placing BID: ${:.2} ({:.1}% below mid)",
                            bid_price_ticks as f64 / 10.0,
                            spread_pct * 100.0
                        ));

                        let _bid_submission = match client
                            .order(market)
                            .buy()
                            .qty(BaseQty::try_from(order_size)?)
                            .limit(Price::ticks(bid_price_ticks))
                            .expires_at(Expiry::from_now(Duration::hours(1)))
                            .post_only()
                            .submit()
                            .await
                        {
                            Ok(sub) => {
                                log_action(&format!("  TX: {}", sub.response().tx_hash));
                                Some(sub)
                            }
                            Err(e) => {
                                log_action(&format!("  Failed: {}", e));
                                None
                            }
                        };

                        // Place ASK order
                        log_action(&format!(
                            "Placing ASK: ${:.2} ({:.1}% above mid)",
                            ask_price_ticks as f64 / 10.0,
                            spread_pct * 100.0
                        ));

                        let _ask_submission = match client
                            .order(market)
                            .sell()
                            .qty(BaseQty::try_from(order_size)?)
                            .limit(Price::ticks(ask_price_ticks))
                            .expires_at(Expiry::from_now(Duration::hours(1)))
                            .post_only()
                            .submit()
                            .await
                        {
                            Ok(sub) => {
                                log_action(&format!("  TX: {}", sub.response().tx_hash));
                                Some(sub)
                            }
                            Err(e) => {
                                log_action(&format!("  Failed: {}", e));
                                None
                            }
                        };

                        // Try WebSocket first, fall back to REST API
                        log_action("Waiting for order confirmations via WebSocket...");

                        let mut new_orders = ActiveOrders::new(current_mid);
                        let target_bid_price = bid_price;
                        let target_ask_price = ask_price;

                        // Listen for order events from WebSocket (with timeout)
                        let mut order_timeout = tokio::time::interval(tokio::time::Duration::from_secs(3));
                        order_timeout.tick().await; // First tick completes immediately

                        'order_wait: loop {
                            tokio::select! {
                                Some(event_result) = stream.next() => {
                                    if let Ok(event) = event_result {
                                        if let WsEvent::Account(account_event) = event {
                                            let event_data = account_event.event.as_value();

                                            // Look for orders array in the event
                                            if let Some(orders_array) = event_data.get("orders").and_then(|v| v.as_array()) {
                                                for order_value in orders_array {
                                                    if let (Some(order_index), Some(price_str), Some(is_ask)) = (
                                                    order_value.get("order_index").and_then(|v| v.as_i64()),
                                                    order_value.get("price").and_then(|v| v.as_str()),
                                                    order_value.get("is_ask").and_then(|v| v.as_bool()),
                                                ) {
                                                    let order_price: f64 = price_str.parse().unwrap_or(0.0);

                                                    // Match BID order
                                                    if !is_ask && (order_price - target_bid_price).abs() < 1.0 && new_orders.bid_order_index.is_none() {
                                                        new_orders.bid_order_index = Some(order_index);
                                                        log_action(&format!("  ✅ BID confirmed via WS: order_index {}", order_index));
                                                    }

                                                    // Match ASK order
                                                    if is_ask && (order_price - target_ask_price).abs() < 1.0 && new_orders.ask_order_index.is_none() {
                                                        new_orders.ask_order_index = Some(order_index);
                                                        log_action(&format!("  ✅ ASK confirmed via WS: order_index {}", order_index));
                                                    }

                                                    // Break if we have both
                                                    if new_orders.has_both_orders() {
                                                        log_action("✅ All orders confirmed via WebSocket!");
                                                        break 'order_wait;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                _ = order_timeout.tick() => {
                                    log_action("⏱️  WS timeout - falling back to REST API...");
                                    break 'order_wait;
                                }
                            }
                        }

                        // If WebSocket didn't get all order indices, use REST API as fallback
                        if !new_orders.has_both_orders() {
                            log_action("Using REST API fallback to fetch order indices...");

                            match client.account().active_orders(market).await {
                                Ok(orders_response) => {
                                    for order in orders_response.orders {
                                        let order_price = order.price.parse::<f64>().unwrap_or(0.0);

                                        // Match BID order
                                        if !order.is_ask && (order_price - target_bid_price).abs() < 1.0 && new_orders.bid_order_index.is_none() {
                                            new_orders.bid_order_index = Some(order.order_index);
                                            log_action(&format!("  ✅ BID confirmed via API: order_index {}", order.order_index));
                                        }

                                        // Match ASK order
                                        if order.is_ask && (order_price - target_ask_price).abs() < 1.0 && new_orders.ask_order_index.is_none() {
                                            new_orders.ask_order_index = Some(order.order_index);
                                            log_action(&format!("  ✅ ASK confirmed via API: order_index {}", order.order_index));
                                        }
                                    }

                                    if new_orders.has_both_orders() {
                                        log_action("✅ All orders confirmed via REST API fallback");
                                    } else {
                                        log_action("⚠️  Warning: Could not find all order indices");
                                    }
                                }
                                Err(e) => {
                                    log_action(&format!("❌ REST API fallback failed: {}", e));
                                }
                            }
                        }

                        active_orders = Some(new_orders);
                        println!();
                    }
                }
            }
                            WsEvent::Account(account_event) => {
                                // Detect order fills via WebSocket
                                let event_data = account_event.event.as_value();

                                if let Some(orders_array) = event_data.get("orders").and_then(|v| v.as_array()) {
                                    if let Some(orders) = &mut active_orders {
                                        for order_value in orders_array {
                                            if let (Some(order_index), Some(status)) = (
                                                order_value.get("order_index").and_then(|v| v.as_i64()),
                                                order_value.get("status").and_then(|v| v.as_str()),
                                            ) {
                                                // Check if our orders got filled
                                                if status == "filled" || status == "cancelled" {
                                                    if Some(order_index) == orders.bid_order_index {
                                                        log_action(&format!("🔔 BID order {} -> {}", order_index, status.to_uppercase()));
                                                        orders.bid_order_index = None;
                                                    }
                                                    if Some(order_index) == orders.ask_order_index {
                                                        log_action(&format!("🔔 ASK order {} -> {}", order_index, status.to_uppercase()));
                                                        orders.ask_order_index = None;
                                                    }
                                                }
                                            }
                                        }

                                        // If either order is gone, we need to refresh
                                        if !orders.has_both_orders() {
                                            log_action("⚠️  Order filled/cancelled - will refresh on next tick");
                                        }
                                    }
                                }
                            }
                            WsEvent::Connected => {
                                log_action("WebSocket connected");
                            }
                            WsEvent::Pong => {
                                // WebSocket protocol pong (handled automatically by SDK)
                                // This event is generated when SDK auto-responds to WebSocket Ping frames
                                log_action("🏓 WebSocket pong (protocol keepalive)");
                            }
                            WsEvent::Unknown(text) => {
                                // Handle JSON ping/pong (application-level, NOT WebSocket protocol)
                                if text.contains("\"type\":\"ping\"") {
                                    log_action("💓 Received JSON ping, sending JSON pong");
                                    // Note: SDK should handle this automatically in handle_text_message
                                    // But we log it here for visibility
                                }
                            }
                            WsEvent::Closed(frame) => {
                                log_action(&format!("WebSocket closed: {:?}", frame));
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                    None => {
                        log_action("❌ WebSocket stream ended unexpectedly");
                        return Ok(());
                    }
                }
            }
            _ = watchdog_interval.tick() => {
                // Check if we've received order book updates recently
                let elapsed = last_orderbook_update.elapsed();
                if elapsed > tokio::time::Duration::from_secs(10) {
                    log_action(&format!("⚠️  WATCHDOG: No order book updates for {:?} - WebSocket may be stale", elapsed));
                    log_action("🔄 Reconnecting WebSocket...");

                    // Cancel existing orders before reconnecting
                    if let Some(orders) = &active_orders {
                        if let Some(bid_index) = orders.bid_order_index {
                            let _ = client.cancel(market, bid_index).submit().await;
                            log_action(&format!("  Cancelled BID (order_index: {}) before reconnect", bid_index));
                        }
                        if let Some(ask_index) = orders.ask_order_index {
                            let _ = client.cancel(market, ask_index).submit().await;
                            log_action(&format!("  Cancelled ASK (order_index: {}) before reconnect", ask_index));
                        }
                    }

                    return Ok(()); // Exit and let user restart
                }
            }
        }

        // Small delay between iterations
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }
}

/// Log an action with timestamp
fn log_action(message: &str) {
    let timestamp = Local::now().format("%H:%M:%S");
    println!("[{}] {}", timestamp, message);
}
