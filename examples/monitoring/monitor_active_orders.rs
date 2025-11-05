// Real-time Trading Dashboard via WebSocket
// Comprehensive account monitor showing:
// 1. Available margin
// 2. Portfolio balance
// 3. Current positions
// 4. Active orders
//
// Notes:
// - Uses lighter_client SDK for WebSocket connections
// - Subscribes to multiple channels: account_all_orders, account_all_positions, user_stats
// - Real-time updates with live dashboard display

use anyhow::Result;
use chrono;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::time::{timeout, Duration};

#[path = "../common/example_context.rs"]
mod common;
use common::ExampleContext;
use lighter_client::ws_client::WsEvent;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Order {
    order_index: Option<u64>,
    order_id: Option<String>,
    client_order_index: Option<u64>,
    client_order_id: Option<String>,
    market_index: Option<u32>,

    // Order details (API uses these exact field names)
    price: Option<String>,                 // e.g., "3200.00"
    initial_base_amount: Option<String>,   // e.g., "0.0050"
    remaining_base_amount: Option<String>, // e.g., "0.0050"
    filled_base_amount: Option<String>,    // e.g., "0.0000"

    // Order properties
    is_ask: Option<bool>, // true = sell, false = buy
    side: Option<String>, // "buy" or "sell" (may be empty)
    #[serde(rename = "type")]
    order_type: Option<String>, // "limit", "market", etc.
    time_in_force: Option<String>, // "good-till-time", "ioc", etc.
    status: Option<String>, // "open", "filled", "cancelled", etc.
    reduce_only: Option<bool>,
}

#[derive(Debug, Clone)]
struct ActiveOrder {
    order_id: String,
    order_index: Option<u64>, // Use this for cancelling orders
    market_id: u32,
    side: String,
    price: f64,
    price_str: String, // Raw price string from WebSocket
    size: f64,
    size_str: String, // Raw size string from WebSocket
    filled: f64,
    filled_str: String, // Raw filled string from WebSocket
    status: String,
    order_type: String,
    time_in_force: String,
    last_update: std::time::Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PositionData {
    // Core fields (from WebSocket docs)
    market_id: Option<u32>,
    symbol: Option<String>,

    // Position data
    sign: Option<i32>,               // Position direction: 1 = long, -1 = short
    position: Option<String>,        // Position size (unsigned, use sign for direction)
    avg_entry_price: Option<String>, // Average entry price
    position_value: Option<String>,  // Current position value

    // PnL fields
    unrealized_pnl: Option<String>,
    realized_pnl: Option<String>,

    // Margin fields
    initial_margin_fraction: Option<String>,
    margin_mode: Option<i32>,
    allocated_margin: Option<String>,
    liquidation_price: Option<String>,

    // Order counts
    open_order_count: Option<u32>,
    pending_order_count: Option<u32>,
    position_tied_order_count: Option<u32>,

    // Optional fields
    total_funding_paid_out: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserStats {
    collateral: Option<String>,
    portfolio_value: Option<String>,
    leverage: Option<String>,
    available_balance: Option<String>,
    margin_usage: Option<String>,
    buying_power: Option<String>,
}

#[derive(Debug, Clone)]
struct Position {
    market_id: u32,
    symbol: String,   // Symbol from WebSocket (e.g., "BTC", "ETH", "SOL")
    size: f64,        // Signed: positive = long, negative = short (for calculations)
    size_str: String, // Raw size string from WebSocket (for display)
    entry_price: f64,
    entry_price_str: String, // Raw entry price string from WebSocket
    unrealized_pnl: f64,
    unrealized_pnl_str: String,   // Raw PnL string from WebSocket
    mark_price: f64,              // Current market price (from position_value / position)
    initial_margin_fraction: f64, // Margin requirement (e.g., 0.02 = 2% = 50x leverage)
    last_update: std::time::Instant,
}

#[derive(Debug, Clone)]
struct AccountInfo {
    available_margin: f64,
    total_balance: f64,
    margin_used: f64,
    unrealized_pnl: f64,
    last_update: std::time::Instant,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt().with_env_filter("info").init();

    println!("\n{}", "═".repeat(80));
    println!("📊 Real-Time Trading Dashboard");
    println!("{}\n", "═".repeat(80));

    // Initialize example context
    let ctx = ExampleContext::initialise(Some("monitor_active_orders")).await?;
    let account_id = ctx.account_id();

    println!("✅ Context initialized\n");

    // Build WebSocket connection with private channel subscriptions
    println!("📡 Subscribing to channels...");
    let mut stream = ctx
        .ws_builder()
        .subscribe_account_all_orders(account_id)
        .subscribe_account_all_positions(account_id)
        .subscribe_user_stats(account_id)
        .connect()
        .await?;

    // Set authentication token for private channels
    let auth_token = ctx.auth_token().await?;
    stream.connection_mut().set_auth_token(auth_token);

    println!("  1️⃣  account_all_orders (active orders)");
    println!("  2️⃣  account_all_positions (positions & PnL)");
    println!("  3️⃣  user_stats (margin & balance)");
    println!("✅ All channels subscribed\n");

    println!("{}", "═".repeat(80));
    println!("🔴 LIVE - Trading Dashboard (Ctrl+C to stop)");
    println!("{}\n", "═".repeat(80));

    // Track dashboard state
    let mut active_orders: HashMap<String, ActiveOrder> = HashMap::new();
    let mut positions: HashMap<u32, Position> = HashMap::new();
    let mut account_info: Option<AccountInfo> = None;
    let mut message_count = 0;
    let mut total_messages_received = 0;
    let mut last_dashboard_update = std::time::Instant::now();

    // Listen for messages
    loop {
        match timeout(Duration::from_secs(30), stream.next()).await {
            Ok(Some(Ok(event))) => {
                total_messages_received += 1;

                match event {
                    WsEvent::Connected => {
                        println!("✅ Connected to WebSocket");
                    }
                    WsEvent::Pong => {
                        // Heartbeat received
                    }
                    WsEvent::Account(envelope) => {
                        message_count += 1;
                        let mut update_dashboard = false;

                        // Parse the account event to determine channel type
                        let event_value = envelope.event.as_value();

                        // Handle account_all_orders channel
                        if let Some(orders_obj) =
                            event_value.get("orders").and_then(|v| v.as_object())
                        {
                            for (_market_id, orders) in orders_obj {
                                if let Some(orders_array) = orders.as_array() {
                                    for order_val in orders_array {
                                        if let Ok(order) =
                                            serde_json::from_value::<Order>(order_val.clone())
                                        {
                                            process_order(&mut active_orders, order, message_count);
                                        }
                                    }
                                }
                            }
                            update_dashboard = true;
                        }

                        // Handle account_all_positions channel
                        if let Some(positions_obj) =
                            event_value.get("positions").and_then(|v| v.as_object())
                        {
                            for (_market_id_key, pos_value) in positions_obj {
                                if let Ok(pos) =
                                    serde_json::from_value::<PositionData>(pos_value.clone())
                                {
                                    process_position(&mut positions, pos);
                                }
                            }
                            update_dashboard = true;
                        }

                        // Handle user_stats channel
                        if let Some(stats_obj) = event_value.get("stats") {
                            if let Ok(user_stats) =
                                serde_json::from_value::<UserStats>(stats_obj.clone())
                            {
                                process_account_data(&mut account_info, user_stats);
                                update_dashboard = true;
                            }
                        }

                        // Update dashboard display every 2 seconds or when data changes
                        if update_dashboard || last_dashboard_update.elapsed().as_secs() >= 2 {
                            display_dashboard(&active_orders, &positions, &account_info);
                            last_dashboard_update = std::time::Instant::now();
                        }
                    }
                    WsEvent::Closed(frame) => {
                        println!("\n❌ WebSocket connection closed: {:?}", frame);
                        break;
                    }
                    WsEvent::Unknown(raw) => {
                        // Skip unknown events
                        if raw.contains("\"type\":\"connected\"")
                            || raw.contains("\"type\":\"subscribed\"")
                        {
                            continue;
                        }
                    }
                    _ => {
                        // Ignore other event types
                    }
                }
            }
            Ok(Some(Err(e))) => {
                println!("\n❌ WebSocket error: {}", e);
                break;
            }
            Ok(None) => {
                println!("\n❌ WebSocket connection closed by server");
                println!(
                    "📊 Session stats: {} total messages, {} order updates processed",
                    total_messages_received, message_count
                );
                break;
            }
            Err(_) => {
                // Timeout - show heartbeat
                println!(
                    "\n💓 Heartbeat ({}s) - {} active orders",
                    30,
                    active_orders.len()
                );
                continue;
            }
        }
    }

    println!("\n✅ Disconnected");

    Ok(())
}

fn process_order(active_orders: &mut HashMap<String, ActiveOrder>, order: Order, msg_num: u32) {
    // Get order ID
    let order_id = match order
        .order_id
        .or_else(|| order.order_index.map(|i| i.to_string()))
    {
        Some(id) => id,
        None => {
            println!("⚠️  Order has no ID, skipping");
            return;
        }
    };

    let status = order.status.as_deref().unwrap_or("unknown");

    // Keep raw strings from WebSocket for exact display
    let price_str = order.price.clone().unwrap_or_else(|| "0".to_string());
    let size_str = order
        .initial_base_amount
        .clone()
        .unwrap_or_else(|| "0".to_string());
    let filled_str = order
        .filled_base_amount
        .clone()
        .unwrap_or_else(|| "0".to_string());

    // Parse numeric fields for calculations (already in decimal format: "3200.00", "0.0050")
    let price = price_str.parse::<f64>().ok().unwrap_or(0.0);
    let size = size_str.parse::<f64>().ok().unwrap_or(0.0);
    let filled = filled_str.parse::<f64>().ok().unwrap_or(0.0);

    // Determine side from is_ask boolean (true = sell, false = buy)
    let side = if let Some(is_ask) = order.is_ask {
        if is_ask {
            "sell"
        } else {
            "buy"
        }
    } else {
        // Fallback to side field if is_ask is missing
        order.side.as_deref().unwrap_or("unknown")
    };

    // Order type and time_in_force are already strings
    let order_type = order.order_type.as_deref().unwrap_or("unknown");
    let tif = order.time_in_force.as_deref().unwrap_or("unknown");

    // Only track ACTIVE orders (not filled/cancelled/expired)
    if status == "open" || status == "partially_filled" {
        let is_new = !active_orders.contains_key(&order_id);

        let active_order = ActiveOrder {
            order_id: order_id.clone(),
            order_index: order.order_index, // Store for cancelling
            market_id: order.market_index.unwrap_or(0),
            side: side.to_string(),
            price,      // f64 for calculations
            price_str,  // Raw string for display
            size,       // f64 for calculations
            size_str,   // Raw string for display
            filled,     // f64 for calculations
            filled_str, // Raw string for display
            status: status.to_string(),
            order_type: order_type.to_string(),
            time_in_force: tif.to_string(),
            last_update: std::time::Instant::now(),
        };

        if is_new {
            println!("\n🔔 New Active Order #{}", msg_num);
            println!("   ID: {}...", &order_id[..order_id.len().min(12)]);
            println!("   {} {} ETH @ ${:.2}", side.to_uppercase(), size, price);
            println!("   Status: {}", status.to_uppercase());
        }

        active_orders.insert(order_id, active_order);
    } else {
        // Remove from active orders if it's filled/cancelled/expired
        if active_orders.remove(&order_id).is_some() {
            println!("\n🔔 Order Closed #{}", msg_num);
            println!("   ID: {}...", &order_id[..order_id.len().min(12)]);
            println!("   Reason: {}", status.to_uppercase());
        }
    }
}

fn process_position(positions: &mut HashMap<u32, Position>, pos_data: PositionData) {
    if let Some(market_id) = pos_data.market_id {
        // Get symbol from WebSocket data
        let symbol = pos_data
            .symbol
            .clone()
            .unwrap_or_else(|| format!("MARKET_{}", market_id));

        // Get sign: 1 = long, -1 = short
        let sign = pos_data.sign.unwrap_or(0);

        // Position size is UNSIGNED - multiply by sign to get signed value
        let position_abs: f64 = pos_data
            .position
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);

        // Calculate SIGNED position size
        let position_size = position_abs * sign as f64;

        // Get average entry price
        let entry_price: f64 = pos_data
            .avg_entry_price
            .as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0.0);

        // Get unrealized PnL
        let unrealized_pnl: f64 = pos_data
            .unrealized_pnl
            .as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0.0);

        if position_abs > 0.0 {
            // Track ANY non-zero position
            // Keep raw strings from WebSocket for exact display
            let size_str_raw = pos_data.position.clone().unwrap_or_else(|| "0".to_string());
            let entry_price_str_raw = pos_data
                .avg_entry_price
                .clone()
                .unwrap_or_else(|| "0".to_string());
            let unrealized_pnl_str_raw = pos_data
                .unrealized_pnl
                .clone()
                .unwrap_or_else(|| "0".to_string());

            // Calculate mark price from position_value / position
            let position_value: f64 = pos_data
                .position_value
                .as_ref()
                .and_then(|p| p.parse().ok())
                .unwrap_or(0.0);
            let mark_price = if position_abs > 0.0 {
                position_value.abs() / position_abs
            } else {
                entry_price // Fallback to entry price
            };

            // Parse initial margin fraction (e.g., "2.00" = 2% = 50x leverage)
            let initial_margin_fraction: f64 = pos_data
                .initial_margin_fraction
                .as_ref()
                .and_then(|p| p.parse().ok())
                .unwrap_or(100.0); // Default to 100% (1x) if missing
            let margin_fraction = initial_margin_fraction / 100.0; // Convert to decimal

            positions.insert(
                market_id,
                Position {
                    market_id,
                    symbol,                                     // Use symbol from WebSocket
                    size: position_size,                        // Signed f64 for calculations
                    size_str: size_str_raw,                     // Raw string for display
                    entry_price,                                // f64 for calculations
                    entry_price_str: entry_price_str_raw,       // Raw string for display
                    unrealized_pnl,                             // f64 for calculations
                    unrealized_pnl_str: unrealized_pnl_str_raw, // Raw string for display
                    mark_price,                                 // Current market price
                    initial_margin_fraction: margin_fraction,   // Margin requirement (decimal)
                    last_update: std::time::Instant::now(),
                },
            );
        } else {
            positions.remove(&market_id); // Remove zero positions
        }
    }
}

fn process_account_data(account_info: &mut Option<AccountInfo>, user_stats: UserStats) {
    let available_margin = user_stats
        .available_balance
        .as_ref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    let total_balance = user_stats
        .portfolio_value
        .as_ref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    let collateral = user_stats
        .collateral
        .as_ref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    let margin_usage = user_stats
        .margin_usage
        .as_ref()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);

    // Calculate margin used from margin_usage percentage
    let margin_used = if margin_usage > 0.0 && total_balance > 0.0 {
        (margin_usage / 100.0) * total_balance
    } else {
        0.0
    };

    // Unrealized PnL = portfolio_value - collateral
    let unrealized_pnl = total_balance - collateral;

    *account_info = Some(AccountInfo {
        available_margin,
        total_balance,
        margin_used,
        unrealized_pnl,
        last_update: std::time::Instant::now(),
    });
}

// Helper: Get market symbol by ID (looks up in positions first, then uses known markets)
fn get_market_symbol(market_id: u32, positions: &HashMap<u32, Position>) -> String {
    // Try to get symbol from positions first
    if let Some(pos) = positions.get(&market_id) {
        return format!("{}-PERP", pos.symbol);
    }

    // Fallback to known markets (ETH and BTC are most common)
    match market_id {
        0 => "ETH-PERP".to_string(),
        1 => "BTC-PERP".to_string(),
        2 => "SOL-PERP".to_string(),
        _ => format!("MARKET_{}-PERP", market_id),
    }
}

fn display_dashboard(
    orders: &HashMap<String, ActiveOrder>,
    positions: &HashMap<u32, Position>,
    account_info: &Option<AccountInfo>,
) {
    // Clear screen for better visualization (optional - comment out if not desired)
    print!("\x1B[2J\x1B[1;1H");

    println!("\n{}", "═".repeat(80));
    println!(
        "📊 TRADING DASHBOARD - {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    println!("{}", "═".repeat(80));

    // ========== 1. ACCOUNT INFO (Margin & Balance) ==========
    println!("\n💰 ACCOUNT SUMMARY");
    println!("{}", "─".repeat(80));

    if let Some(acc) = account_info {
        println!("  Available Margin:  ${:>12.2}", acc.available_margin);
        println!("  Total Balance:     ${:>12.2}", acc.total_balance);
        println!("  Margin Used:       ${:>12.2}", acc.margin_used);

        let margin_ratio = if acc.total_balance > 0.0 {
            (acc.margin_used / acc.total_balance) * 100.0
        } else {
            0.0
        };
        println!("  Margin Ratio:       {:>11.1}%", margin_ratio);

        let pnl_color = if acc.unrealized_pnl >= 0.0 {
            "🟢"
        } else {
            "🔴"
        };
        println!(
            "  Unrealized PnL:    {}${:>11.2}",
            pnl_color, acc.unrealized_pnl
        );

        // ========== MAX ORDER SIZE CALCULATOR ==========
        println!(
            "\n💵 MAX ORDER SIZE (with available margin ${:.2})",
            acc.available_margin
        );
        println!("   Note: Based on YOUR CURRENT leverage settings (change on website for more)");
        println!("{}", "─".repeat(80));

        // Collect markets from positions (these have live prices and margin requirements)
        let mut market_data: Vec<_> = positions.values().collect();
        market_data.sort_by_key(|p| p.market_id);

        if market_data.is_empty() {
            println!("  (no position data available for calculation)");
        } else {
            for pos in market_data {
                // Max Position Value = Available Margin × Leverage
                // Max Size = Max Position Value / Current Price
                let current_leverage = 1.0 / pos.initial_margin_fraction;
                let max_position_value = acc.available_margin * current_leverage;
                let max_size = max_position_value / pos.mark_price;

                println!("\n  {} (Market ID: {})", pos.symbol, pos.market_id);
                println!("    Current Price:     ${:>12.2}", pos.mark_price);
                println!(
                    "    Your Leverage:      {:>11.0}x (margin: {:.2}%)",
                    current_leverage,
                    pos.initial_margin_fraction * 100.0
                );
                println!("    Max Position Size:  {:>12.4} contracts", max_size);
                println!("    Max Notional:      ${:>12.2}", max_position_value);
            }
        }
    } else {
        println!("  (waiting for account data...)");
    }

    // ========== 2. POSITIONS ==========
    println!("\n📍 OPEN POSITIONS ({} total)", positions.len());
    println!("{}", "─".repeat(80));

    if positions.is_empty() {
        println!("  (no open positions)");
    } else {
        for (_, pos) in positions.iter() {
            // Use symbol from WebSocket data (e.g., "BTC", "ETH", "SOL")
            // Add "-PERP" suffix for display (all are perpetual futures)
            let market_name = format!("{}-PERP", pos.symbol);

            let side_indicator = if pos.size > 0.0 {
                "🟢 LONG"
            } else {
                "🔴 SHORT"
            };
            let pnl_indicator = if pos.unrealized_pnl >= 0.0 {
                "🟢"
            } else {
                "🔴"
            };

            println!("\n  {} {}", side_indicator, market_name);

            // Display raw strings from WebSocket (exact precision)
            println!("    Size:         {} contracts", pos.size_str);
            println!("    Entry Price:  ${}", pos.entry_price_str);
            println!(
                "    Unreal PnL:   {}${}",
                pnl_indicator, pos.unrealized_pnl_str
            );
        }
    }

    // ========== 3. ACTIVE ORDERS ==========
    println!("\n📋 ACTIVE ORDERS ({} total)", orders.len());
    println!("{}", "─".repeat(80));

    if orders.is_empty() {
        println!("  (no active orders)");
    } else {
        let mut sorted: Vec<_> = orders.values().collect();
        sorted.sort_by_key(|o| std::cmp::Reverse(o.last_update));

        for order in sorted.iter() {
            // Use helper to get market symbol (looks up from positions or uses fallback)
            let market_name = get_market_symbol(order.market_id, positions);

            let side_indicator = if order.side == "buy" {
                "🟢 BUY"
            } else {
                "🔴 SELL"
            };

            println!(
                "\n  {} {} - ID: {}...",
                side_indicator,
                market_name,
                &order.order_id[..order.order_id.len().min(12)]
            );

            // Show order_index for cancelling
            if let Some(idx) = order.order_index {
                println!("    Index:   {}  ⚠️  USE THIS TO CANCEL!", idx);
                println!("    Market:  {} (use in cancel command)", order.market_id);
            }

            // Display raw strings from WebSocket (exact precision)
            println!(
                "    Price:   ${}  |  Size: {}",
                order.price_str, order.size_str
            );

            if order.filled > 0.0 {
                let fill_pct = (order.filled / order.size) * 100.0;
                println!("    Filled:  {} ({:.1}%)", order.filled_str, fill_pct);
            }

            println!(
                "    Type:    {} ({})",
                order.order_type.to_uppercase(),
                order.time_in_force.to_uppercase()
            );
        }
    }

    println!("\n{}", "═".repeat(80));
}
