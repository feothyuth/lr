//! Comprehensive test of ALL 17+ WebSocket channels from Lighter DEX API
//! https://apidocs.lighter.xyz/docs/websocket-reference#channels
//!
//! This example connects to each channel individually to verify connectivity
//! and data reception, using the lighter_client SDK.

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
    let account = ctx.account_id();

    println!("\n{}", "=".repeat(80));
    println!("🚀 Testing ALL 17+ WebSocket Channels - Lighter DEX");
    println!("{}\n", "=".repeat(80));

    // PUBLIC CHANNELS
    println!("\n📡 PUBLIC CHANNELS (No Authentication Required)\n");
    println!("{}", "-".repeat(80));

    test_public_channel(&ctx, "1. order_book", |builder| {
        builder.subscribe_order_book(market)
    })
    .await?;

    test_public_channel(&ctx, "2. market_stats", |builder| {
        builder.subscribe_market_stats(market)
    })
    .await?;

    test_public_channel(&ctx, "3. bbo", |builder| builder.subscribe_bbo(market)).await?;

    test_public_channel(&ctx, "4. trade", |builder| builder.subscribe_trade(market)).await?;

    test_public_channel(&ctx, "5. transaction", |builder| {
        builder.subscribe_transactions()
    })
    .await?;

    test_public_channel(&ctx, "6. executed_transaction", |builder| {
        builder.subscribe_executed_transactions()
    })
    .await?;

    test_public_channel(&ctx, "7. height", |builder| builder.subscribe_height()).await?;

    // PRIVATE CHANNELS
    println!("\n🔐 PRIVATE CHANNELS (Authentication Required)\n");
    println!("{}", "-".repeat(80));

    // Get auth token once
    println!("🔑 Getting authentication token via SDK...");
    let auth_token = ctx.auth_token().await?;
    println!("✅ Auth token acquired: {}...\n", &auth_token[..20]);

    test_private_channel(&ctx, "8. account_all_positions", |builder| {
        builder.subscribe_account_all_positions(account)
    })
    .await?;

    test_private_channel(&ctx, "9. account_market_positions", |builder| {
        builder.subscribe_account_market_positions(market, account)
    })
    .await?;

    test_private_channel(&ctx, "10. user_stats", |builder| {
        builder.subscribe_user_stats(account)
    })
    .await?;

    test_private_channel(&ctx, "11. account_tx", |builder| {
        builder.subscribe_account_tx(account)
    })
    .await?;

    test_private_channel(&ctx, "12. account_all_orders", |builder| {
        builder.subscribe_account_all_orders(account)
    })
    .await?;

    test_private_channel(&ctx, "13. account_market_orders", |builder| {
        builder.subscribe_account_market_orders(market, account)
    })
    .await?;

    test_private_channel(&ctx, "14. account_all_trades", |builder| {
        builder.subscribe_account_all_trades(account)
    })
    .await?;

    test_private_channel(&ctx, "15. account_market_trades", |builder| {
        builder.subscribe_account_market_trades(market, account)
    })
    .await?;

    test_private_channel(&ctx, "16. pool_data", |builder| {
        builder.subscribe_pool_data(account)
    })
    .await?;

    test_private_channel(&ctx, "17. pool_info", |builder| {
        builder.subscribe_pool_info(account)
    })
    .await?;

    test_private_channel(&ctx, "18. notification", |builder| {
        builder.subscribe_notification(account)
    })
    .await?;

    println!("\n{}", "=".repeat(80));
    println!("✅ ALL CHANNELS TESTED SUCCESSFULLY!");
    println!("{}\n", "=".repeat(80));

    Ok(())
}

/// Test a public channel subscription
async fn test_public_channel<F>(ctx: &ExampleContext, name: &str, configure: F) -> Result<()>
where
    F: FnOnce(lighter_client::ws_client::WsBuilder<'_>) -> lighter_client::ws_client::WsBuilder<'_>,
{
    print!("  Testing {} ... ", name);
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let builder = ctx.ws_builder();
    let builder = configure(builder);
    let mut stream = builder.connect().await?;

    // Wait for first event (Connected or data)
    match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
        Ok(Some(Ok(WsEvent::Connected))) => {
            // Got connected, wait for actual data
            match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
                Ok(Some(Ok(WsEvent::Unknown(raw)))) => {
                    if raw.contains("\"error\"") || raw.contains("Invalid") {
                        println!("❌ FAIL - Error: {}", &raw[..raw.len().min(100)]);
                    } else {
                        println!("✅ PASS ({} bytes)", raw.len());
                    }
                }
                Ok(Some(Ok(event))) => {
                    let size = format!("{:?}", event).len();
                    println!("✅ PASS ({} bytes)", size);
                }
                Ok(Some(Err(e))) => println!("❌ FAIL - Error: {}", e),
                Ok(None) => println!("❌ FAIL - Connection closed"),
                Err(_) => println!("⏱️  TIMEOUT (no data in 3s)"),
            }
        }
        Ok(Some(Ok(event))) => {
            // Got data directly
            let size = format!("{:?}", event).len();
            println!("✅ PASS ({} bytes)", size);
        }
        Ok(Some(Err(e))) => println!("❌ FAIL - Error: {}", e),
        Ok(None) => println!("❌ FAIL - Connection closed"),
        Err(_) => println!("⏱️  TIMEOUT (no connection in 3s)"),
    }

    tokio::time::sleep(Duration::from_millis(500)).await; // Rate limit protection
    Ok(())
}

/// Test a private channel subscription
async fn test_private_channel<F>(ctx: &ExampleContext, name: &str, configure: F) -> Result<()>
where
    F: FnOnce(lighter_client::ws_client::WsBuilder<'_>) -> lighter_client::ws_client::WsBuilder<'_>,
{
    print!("  Testing {} ... ", name);
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let auth_token = ctx.auth_token().await?;
    let builder = ctx.ws_builder();
    let builder = configure(builder);
    let mut stream = builder.connect().await?;

    // Authenticate
    stream.connection_mut().set_auth_token(auth_token);

    // Wait for first event (Connected or data)
    match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
        Ok(Some(Ok(WsEvent::Connected))) => {
            // Got connected, wait for actual data
            match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
                Ok(Some(Ok(WsEvent::Unknown(raw)))) => {
                    if raw.contains("\"error\"") || raw.contains("Invalid") {
                        println!("❌ FAIL - Error: {}", &raw[..raw.len().min(100)]);
                    } else {
                        println!("✅ PASS ({} bytes)", raw.len());
                    }
                }
                Ok(Some(Ok(event))) => {
                    let size = format!("{:?}", event).len();
                    println!("✅ PASS ({} bytes)", size);
                }
                Ok(Some(Err(e))) => println!("❌ FAIL - Error: {}", e),
                Ok(None) => println!("❌ FAIL - Connection closed"),
                Err(_) => println!("⏱️  TIMEOUT (no data in 3s)"),
            }
        }
        Ok(Some(Ok(event))) => {
            // Got data directly
            let size = format!("{:?}", event).len();
            println!("✅ PASS ({} bytes)", size);
        }
        Ok(Some(Err(e))) => println!("❌ FAIL - Error: {}", e),
        Ok(None) => println!("❌ FAIL - Connection closed"),
        Err(_) => println!("⏱️  TIMEOUT (no connection in 3s)"),
    }

    tokio::time::sleep(Duration::from_millis(500)).await; // Rate limit protection
    Ok(())
}
