// Multi-Channel WebSocket Monitor
// Demonstrates subscribing to MULTIPLE channels on a SINGLE WebSocket connection
// Shows order book, trades, positions, and orders all in one connection

use anyhow::Result;
use futures_util::StreamExt;
use tokio::time::{timeout, Duration};

#[path = "../common/example_context.rs"]
mod common;
use common::ExampleContext;
use lighter_client::ws_client::WsEvent;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt().with_env_filter("info").init();

    println!("\n{}", "═".repeat(80));
    println!("📡 Multi-Channel WebSocket Monitor");
    println!("   Subscribing to 5 channels on 1 connection");
    println!("{}\n", "═".repeat(80));

    // Initialize example context
    let ctx = ExampleContext::initialise(Some("multi_channel_monitor")).await?;
    let account_id = ctx.account_id();
    let market = ctx.market_id();
    let market_index: i32 = market.into();

    println!("✅ Context initialized\n");

    println!("{}", "═".repeat(80));
    println!("📡 Subscribing to MULTIPLE channels on this ONE connection:");
    println!("{}", "═".repeat(80));

    // Build WebSocket connection with multiple channel subscriptions
    println!("1️⃣  Subscribing to order_book (market {})...", market_index);
    println!("2️⃣  Subscribing to recent trades...");
    println!("3️⃣  Subscribing to account_all_positions...");
    println!("4️⃣  Subscribing to account_all_orders...");
    println!("5️⃣  Subscribing to account_all_trades...");

    let mut stream = ctx
        .ws_builder()
        .subscribe_order_book(market)
        .subscribe_trade(market)
        .subscribe_account_all_positions(account_id)
        .subscribe_account_all_orders(account_id)
        .subscribe_account_all_trades(account_id)
        .connect()
        .await?;

    // Set authentication token for private channels
    let auth_token = ctx.auth_token().await?;
    stream.connection_mut().set_auth_token(auth_token);

    println!("   ✅ All subscriptions complete\n");

    println!("{}", "═".repeat(80));
    println!(
        "✅ All 5 channels subscribed on 1 WebSocket connection for market {}!",
        market_index
    );
    println!("{}", "═".repeat(80));
    println!("\n🔴 LIVE - Monitoring all channels (Ctrl+C to stop)\n");

    let mut message_count = 0;

    // Listen for messages from ALL channels on this ONE connection
    loop {
        match timeout(Duration::from_secs(30), stream.next()).await {
            Ok(Some(Ok(event))) => {
                message_count += 1;

                match event {
                    WsEvent::Connected => {
                        println!("✅ Connected to WebSocket");
                    }
                    WsEvent::Pong => {
                        println!("💓 Ping/Pong heartbeat");
                    }
                    WsEvent::OrderBook(order_book_event) => {
                        println!("\n📊 [ORDER BOOK] Update #{}", message_count);
                        let bids = &order_book_event.state.bids;
                        if let Some(best_bid) = bids.first() {
                            if let Ok(bid_price) = best_bid.price.parse::<f64>() {
                                println!("   Best Bid: ${:.2}", bid_price);
                            }
                        }
                    }
                    WsEvent::Trade(trade_event) => {
                        println!("\n📈 [TRADE] Update #{}", message_count);
                        println!("   Market: {}", trade_event.channel);
                    }
                    WsEvent::Account(envelope) => {
                        let event_value = envelope.event.as_value();

                        // Determine which account channel this is from
                        if event_value.get("positions").is_some() {
                            println!("\n💼 [POSITIONS] Update #{}", message_count);
                            println!("   Position changed");
                        } else if event_value.get("orders").is_some() {
                            println!("\n📋 [ORDERS] Update #{}", message_count);
                            println!("   Order state changed");
                        } else if event_value.get("trades").is_some()
                            || event_value.get("trade").is_some()
                        {
                            println!("\n✅ [FILLS] Update #{}", message_count);
                            println!("   Order filled");
                        } else {
                            println!("\n❓ [ACCOUNT] Update #{}", message_count);
                            println!("   Unknown account event type");
                        }
                    }
                    WsEvent::Closed(frame) => {
                        println!("\n❌ WebSocket connection closed: {:?}", frame);
                        break;
                    }
                    WsEvent::Unknown(raw) => {
                        // Skip connection messages
                        if raw.contains("\"type\":\"connected\"")
                            || raw.contains("\"type\":\"subscribed\"")
                        {
                            continue;
                        }
                        println!("\n❓ [UNKNOWN] Update #{}", message_count);
                        println!("   {}", &raw[..raw.len().min(100)]);
                    }
                    _ => {
                        // Other event types
                    }
                }
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
                println!(
                    "\n💓 Heartbeat (30s timeout) - {} messages received",
                    message_count
                );
                continue;
            }
        }
    }

    println!("\n✅ Disconnected");

    Ok(())
}
