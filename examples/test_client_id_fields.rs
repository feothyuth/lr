//! Simple test to check if ask_client_id and bid_client_id exist in trade data

#[path = "common/example_context.rs"]
mod common;

use anyhow::Result;
use common::ExampleContext;
use futures_util::StreamExt;
use lighter_client::ws_client::WsEvent;

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("test_client_id")).await?;
    let market = ctx.market_id();
    let account = ctx.account_id();

    println!("\n════════════════════════════════════════════════════════════════");
    println!("🔍 Testing for ask_client_id and bid_client_id fields");
    println!("════════════════════════════════════════════════════════════════\n");

    println!("📡 Subscribing to account_market_trades channel");
    println!("   Market: {}", market.into_inner());
    println!("   Account: {}", account.into_inner());
    println!("\n⏳ Waiting for trades...\n");
    println!("💡 TIP: Trades happen automatically when market moves\n");

    let mut stream = ctx
        .ws_builder()
        .subscribe_account_market_trades(market, account)
        .connect()
        .await?;

    let mut trade_count = 0;

    while let Some(event) = stream.next().await {
        match event? {
            WsEvent::Connected => {
                println!("✅ Connected to Lighter WebSocket\n");
            }
            WsEvent::Account(envelope) => {
                println!("📥 Received Account event for account: {}", envelope.account.into_inner());

                if envelope.account != account {
                    println!("   ⚠️  Different account, skipping\n");
                    continue;
                }

                // Get raw JSON
                let raw_json = envelope.event.into_inner();

                println!("📄 Raw event JSON:");
                println!("{}\n", serde_json::to_string_pretty(&raw_json)
                    .unwrap_or_else(|_| "parse error".to_string()));

                // Check if it contains trades array
                if let Some(trades_array) = raw_json.get("trades") {
                    if let Some(arr) = trades_array.as_array() {
                        for trade in arr {
                            trade_count += 1;

                            println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                            println!("📊 TRADE #{}", trade_count);
                            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

                            // Print the full trade JSON
                            println!("📄 Full trade object:");
                            println!("{}", serde_json::to_string_pretty(&trade)
                                .unwrap_or_else(|_| "parse error".to_string()));

                            // Check for client_id fields
                            println!("\n🔍 Checking for client_id fields:");

                            if let Some(ask_client_id) = trade.get("ask_client_id") {
                                println!("   ✅ ask_client_id: {}", ask_client_id);
                            } else {
                                println!("   ❌ ask_client_id: NOT FOUND");
                            }

                            if let Some(bid_client_id) = trade.get("bid_client_id") {
                                println!("   ✅ bid_client_id: {}", bid_client_id);
                            } else {
                                println!("   ❌ bid_client_id: NOT FOUND");
                            }

                            // Show other useful fields
                            println!("\n📋 Other fields:");
                            if let Some(price) = trade.get("price") {
                                println!("   Price: {}", price);
                            }
                            if let Some(size) = trade.get("size") {
                                println!("   Size: {}", size);
                            }
                            if let Some(ask_id) = trade.get("ask_id") {
                                println!("   ask_id (order index): {}", ask_id);
                            }
                            if let Some(bid_id) = trade.get("bid_id") {
                                println!("   bid_id (order index): {}", bid_id);
                            }
                            if let Some(ask_account) = trade.get("ask_account_id") {
                                println!("   ask_account_id: {}", ask_account);
                            }
                            if let Some(bid_account) = trade.get("bid_account_id") {
                                println!("   bid_account_id: {}", bid_account);
                            }

                            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
                        }
                    }
                }
            }
            WsEvent::Pong => {}
            other => {
                println!("ℹ️  Other event: {:?}\n", other);
            }
        }
    }

    Ok(())
}
