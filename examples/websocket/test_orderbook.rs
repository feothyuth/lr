//! Test real-time order book updates via WebSocket
//! Subscribes to order_book channel and displays live updates using the lighter_client SDK

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::ExampleContext;
use futures_util::StreamExt;
use lighter_client::ws_client::WsEvent;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(None).await?;
    let market = ctx.market_id();

    println!("\n{}", "=".repeat(80));
    println!(
        "📊 Real-Time Order Book Updates - Market ID {}",
        market.into_inner()
    );
    println!("{}\n", "=".repeat(80));

    // Connect to WebSocket with order book subscription
    println!("🌐 Connecting to WebSocket...");
    let mut stream = ctx
        .ws_builder()
        .subscribe_order_book(market)
        .connect()
        .await?;
    println!("✅ Connected\n");

    println!("📈 Listening for order book updates (Ctrl+C to stop)...\n");
    println!("{}", "-".repeat(80));

    let mut message_count = 0;

    // Listen for messages
    loop {
        match tokio::time::timeout(Duration::from_secs(30), stream.next()).await {
            Ok(Some(Ok(WsEvent::Connected))) => {
                println!("🔗 WebSocket connected");
                continue;
            }
            Ok(Some(Ok(WsEvent::OrderBook(order_book)))) => {
                message_count += 1;

                println!("\n📊 Order Book Update #{}", message_count);
                println!("{}", "-".repeat(80));

                let bids = &order_book.state.bids;
                let asks = &order_book.state.asks;

                // Display top 3 bids
                println!("📗 Bids (top 3):");
                for (i, bid) in bids.iter().take(3).enumerate() {
                    println!("   {} - ${} × {} base units", i + 1, bid.price, bid.size);
                }

                // Display top 3 asks
                println!("\n📕 Asks (top 3):");
                for (i, ask) in asks.iter().take(3).enumerate() {
                    println!("   {} - ${} × {} base units", i + 1, ask.price, ask.size);
                }

                // Calculate spread
                if let (Some(best_bid), Some(best_ask)) = (bids.first(), asks.first()) {
                    if let (Ok(bid_price), Ok(ask_price)) =
                        (best_bid.price.parse::<f64>(), best_ask.price.parse::<f64>())
                    {
                        let spread = ask_price - bid_price;
                        let spread_bps = (spread / bid_price) * 10000.0;
                        println!("\n💰 Spread: ${:.2} ({:.2} bps)", spread, spread_bps);
                    }
                }

                println!("{}", "-".repeat(80));
            }
            Ok(Some(Ok(WsEvent::Pong))) => {
                // Silently handle pings/pongs
                continue;
            }
            Ok(Some(Ok(WsEvent::Unknown(raw)))) => {
                // Handle unknown events - might be subscription confirmations
                if raw.contains("\"type\":\"connected\"") || raw.contains("\"type\":\"subscribed\"")
                {
                    continue;
                }

                // Parse as JSON to display order book data from unknown format
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&raw) {
                    message_count += 1;

                    println!("\n📊 Order Book Update #{}", message_count);
                    println!("{}", "-".repeat(80));

                    // Extract bids and asks
                    if let Some(bids) = data["bids"].as_array() {
                        println!("📗 Bids (top 3):");
                        for (i, bid) in bids.iter().take(3).enumerate() {
                            if let (Some(price), Some(size)) = (
                                bid.get(0).and_then(|v| v.as_f64()),
                                bid.get(1).and_then(|v| v.as_f64()),
                            ) {
                                println!("   {} - ${:.2} × {:.4} units", i + 1, price, size);
                            }
                        }
                    }

                    if let Some(asks) = data["asks"].as_array() {
                        println!("\n📕 Asks (top 3):");
                        for (i, ask) in asks.iter().take(3).enumerate() {
                            if let (Some(price), Some(size)) = (
                                ask.get(0).and_then(|v| v.as_f64()),
                                ask.get(1).and_then(|v| v.as_f64()),
                            ) {
                                println!("   {} - ${:.2} × {:.4} units", i + 1, price, size);
                            }
                        }
                    }

                    // Calculate spread
                    if let (Some(bids), Some(asks)) =
                        (data["bids"].as_array(), data["asks"].as_array())
                    {
                        if let (Some(best_bid), Some(best_ask)) = (
                            bids.first().and_then(|b| b.get(0)).and_then(|v| v.as_f64()),
                            asks.first().and_then(|a| a.get(0)).and_then(|v| v.as_f64()),
                        ) {
                            let spread = best_ask - best_bid;
                            let spread_bps = (spread / best_bid) * 10000.0;
                            println!("\n💰 Spread: ${:.2} ({:.2} bps)", spread, spread_bps);
                        }
                    }

                    println!("{}", "-".repeat(80));
                }
            }
            Ok(Some(Ok(other))) => {
                // Display other events
                common::display_ws_event(other);
            }
            Ok(Some(Err(e))) => {
                println!("\n❌ WebSocket error: {}", e);
                break;
            }
            Ok(None) => {
                println!("\n❌ WebSocket connection closed");
                break;
            }
            Err(_) => {
                println!("\n⏱️  No updates for 30 seconds");
                continue;
            }
        }
    }

    println!("\n✅ Disconnected");

    Ok(())
}
