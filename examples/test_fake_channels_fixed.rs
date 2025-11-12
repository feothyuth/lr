//! Live monitoring test for all 3 fake channel fixes
//!
//! Monitors continuously:
//! 1. subscribe_bbo() - synthetic BBO from order book
//! 2. subscribe_account_market_positions() - filtered positions by market
//! 3. subscribe_account_market_trades() - filtered trades by market

#[path = "common/example_context.rs"]
mod common;

use anyhow::Result;
use common::ExampleContext;
use futures_util::StreamExt;
use lighter_client::ws_client::{AccountEvent, TradeSide, WsEvent};

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("test_fake_channels")).await?;
    let market = ctx.market_id();
    let account = ctx.account_id();

    println!("\n{}", "═".repeat(80));
    println!("🔴 LIVE MONITORING - All 3 Fake Channel Fixes");
    println!("{}", "═".repeat(80));
    println!("Market: {}", market.into_inner());
    println!("Account: {}", account.into_inner());
    println!();
    println!("Monitoring:");
    println!("  1️⃣  BBO (synthetic from order_book)");
    println!("  2️⃣  Account Positions (filtered by market)");
    println!("  3️⃣  Account Trades (filtered by market)");
    println!();
    println!("Press Ctrl+C to stop");
    println!("{}", "═".repeat(80));
    println!();

    // Create combined stream with all 3 channels
    // Auth token is automatically fetched when private channels are subscribed
    let mut stream = ctx
        .ws_builder()
        .subscribe_order_book(market)
        .subscribe_bbo(market)
        .subscribe_account_market_positions(market, account)
        .subscribe_account_market_trades(market, account)
        .connect()
        .await?;

    let mut bbo_count = 0u32;
    let mut bbo_changes = 0u32;
    let mut ob_updates = 0u32;
    let mut position_count = 0u32;
    let mut trade_count = 0u32;
    let mut last_bid = String::new();
    let mut last_ask = String::new();

    println!("🟢 Connected - Monitoring live events...\n");

    loop {
        match stream.next().await {
            Some(Ok(event)) => match event {
                WsEvent::Connected => {
                    let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
                    println!("[{}] 🔗 WebSocket connected", timestamp);
                }
                WsEvent::BBO(bbo) => {
                    bbo_count += 1;
                    let bid = bbo.best_bid.as_deref().unwrap_or("N/A");
                    let ask = bbo.best_ask.as_deref().unwrap_or("N/A");

                    // Check if BBO changed
                    if bid != last_bid || ask != last_ask {
                        bbo_changes += 1;
                        let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");

                        // Use SDK helper methods
                        let spread = bbo.spread()
                            .map(|s| format!("{:.2}", s))
                            .unwrap_or_else(|| "?".to_string());

                        println!("[{}] 1️⃣  BBO #{:04} (change #{:04}) │ Bid: {:>10} │ Ask: {:>10} │ Spread: {:>8}",
                            timestamp, bbo_count, bbo_changes, bid, ask, spread);
                        last_bid = bid.to_string();
                        last_ask = ask.to_string();
                    }
                }
                WsEvent::OrderBook(_ob) => {
                    ob_updates += 1;
                    // Silently track order book updates
                }
                WsEvent::Account(envelope) => {
                    let event_data = envelope.event.as_value();
                    let channel = event_data
                        .get("channel")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");

                    if channel.contains("account_positions") {
                        position_count += 1;
                        let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
                        println!("[{}] 2️⃣  Position #{:04} │ Channel: {}", timestamp, position_count, channel);

                        // Show position details using SDK helpers
                        if let Some(positions) = event_data.get("positions") {
                            if let Some(obj) = positions.as_object() {
                                for (market_id, pos_data) in obj {
                                    if let Some(position_size) = pos_data.get("position").and_then(|v| v.as_str()) {
                                        let side = AccountEvent::position_side(pos_data)
                                            .unwrap_or("?");
                                        println!("           Market {} │ {} │ Size: {}",
                                            market_id, side, position_size);
                                    }
                                }
                            }
                        }
                    } else if channel.contains("account_trades") {
                        trade_count += 1;
                        let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
                        println!("[{}] 3️⃣  Trade #{:04} │ Channel: {}", timestamp, trade_count, channel);

                        // Show trade details using SDK helpers
                        if let Some(trades_by_market) = envelope.event.get_trades_by_market() {
                            for (market_id, trades_array) in trades_by_market {
                                if let Some(arr) = trades_array.as_array() {
                                    println!("           Market {} │ {} trades", market_id, arr.len());
                                    for trade in arr.iter().take(3) {
                                        // Use SDK helper to determine trade side
                                        let side = envelope.event.trade_side_for_account(trade, account)
                                            .map(|s| match s {
                                                TradeSide::Buy => "BUY",
                                                TradeSide::Sell => "SELL",
                                            })
                                            .unwrap_or("?");

                                        let price = trade.get("price")
                                            .and_then(|v| v.as_str()).unwrap_or("?");
                                        let size = trade.get("size")
                                            .and_then(|v| v.as_str()).unwrap_or("?");

                                        println!("             {} │ {} @ {}", side, size, price);
                                    }
                                }
                            }
                        }
                    }
                }
                WsEvent::Pong => {
                    // Keep-alive, no output
                }
                WsEvent::Closed(frame) => {
                    println!("\n🔌 Connection closed: {:?}", frame);
                    break;
                }
                WsEvent::Unknown(msg) => {
                    // Parse and display error details
                    if msg.contains("Invalid Channel") {
                        eprintln!("⚠️  Server rejected a channel subscription");
                        eprintln!("   Message: {}", msg);
                        eprintln!("   This might mean:");
                        eprintln!("   - Authentication issue");
                        eprintln!("   - Channel doesn't exist");
                        eprintln!("   - Duplicate subscription attempt");
                    } else {
                        println!("❓ Unknown: {}", msg);
                    }
                }
                other => {
                    // Log unexpected events
                    common::display_ws_event(other);
                }
            },
            Some(Err(e)) => {
                eprintln!("❌ Error: {}", e);
                break;
            }
            None => {
                println!("\n✅ Stream ended");
                break;
            }
        }
    }

    println!("\n{}", "═".repeat(80));
    println!("📊 Final Stats:");
    println!("   BBO Events: {}", bbo_count);
    println!("   Position Updates: {}", position_count);
    println!("   Trade Events: {}", trade_count);
    println!("{}", "═".repeat(80));

    Ok(())
}
