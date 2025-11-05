//! Comprehensive test of ALL 17+ WebSocket channels from Lighter DEX API
//! This is a more streamlined version that tests all channels sequentially
//! using the lighter_client SDK.

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

    // PUBLIC CHANNELS (7 channels)
    println!("\n📡 PUBLIC CHANNELS (No Authentication)\n");

    test_public("1. order_book", &ctx, market, |b| {
        b.subscribe_order_book(market)
    })
    .await?;

    test_public("2. market_stats", &ctx, market, |b| {
        b.subscribe_market_stats(market)
    })
    .await?;

    test_public("3. bbo", &ctx, market, |b| b.subscribe_bbo(market)).await?;

    test_public("4. trade", &ctx, market, |b| b.subscribe_trade(market)).await?;

    test_public("5. transaction", &ctx, market, |b| {
        b.subscribe_transactions()
    })
    .await?;

    test_public("6. executed_transaction", &ctx, market, |b| {
        b.subscribe_executed_transactions()
    })
    .await?;

    test_public("7. height", &ctx, market, |b| b.subscribe_height()).await?;

    // PRIVATE CHANNELS (10+ channels)
    println!("\n🔐 PRIVATE CHANNELS (Authentication Required)\n");

    // Get auth token
    println!("🔑 Getting authentication token...");
    let _auth_token = ctx.auth_token().await?;
    println!("✅ Token acquired\n");

    test_private("8. account_all_positions", &ctx, market, account, |b| {
        b.subscribe_account_all_positions(account)
    })
    .await?;

    test_private("9. account_market_positions", &ctx, market, account, |b| {
        b.subscribe_account_market_positions(market, account)
    })
    .await?;

    test_private("10. user_stats", &ctx, market, account, |b| {
        b.subscribe_user_stats(account)
    })
    .await?;

    test_private("11. account_tx", &ctx, market, account, |b| {
        b.subscribe_account_tx(account)
    })
    .await?;

    test_private("12. account_all_orders", &ctx, market, account, |b| {
        b.subscribe_account_all_orders(account)
    })
    .await?;

    test_private("13. account_market_orders", &ctx, market, account, |b| {
        b.subscribe_account_market_orders(market, account)
    })
    .await?;

    test_private("14. account_all_trades", &ctx, market, account, |b| {
        b.subscribe_account_all_trades(account)
    })
    .await?;

    test_private("15. account_market_trades", &ctx, market, account, |b| {
        b.subscribe_account_market_trades(market, account)
    })
    .await?;

    test_private("16. pool_data", &ctx, market, account, |b| {
        b.subscribe_pool_data(account)
    })
    .await?;

    test_private("17. pool_info", &ctx, market, account, |b| {
        b.subscribe_pool_info(account)
    })
    .await?;

    test_private("18. notification", &ctx, market, account, |b| {
        b.subscribe_notification(account)
    })
    .await?;

    println!("\n{}", "=".repeat(80));
    println!("✅ ALL CHANNELS TESTED!");
    println!("{}\n", "=".repeat(80));

    Ok(())
}

/// Test a public channel
async fn test_public<F>(
    name: &str,
    ctx: &ExampleContext,
    _market: lighter_client::types::MarketId,
    configure: F,
) -> Result<()>
where
    F: FnOnce(lighter_client::ws_client::WsBuilder<'_>) -> lighter_client::ws_client::WsBuilder<'_>,
{
    print!("  Testing {} ... ", name);
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let builder = ctx.ws_builder();
    let builder = configure(builder);
    let mut stream = builder.connect().await?;

    // Wait for first message (Connected or data)
    match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
        Ok(Some(Ok(WsEvent::Connected))) => {
            // Got connected, wait for data
            match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
                Ok(Some(Ok(WsEvent::Unknown(raw)))) => {
                    if raw.contains("\"error\"") {
                        println!("❌ FAIL - Error");
                    } else {
                        println!("✅ PASS ({} bytes)", raw.len());
                    }
                }
                Ok(Some(Ok(_))) => println!("✅ PASS"),
                Ok(Some(Err(_))) => println!("❌ FAIL - Error"),
                Ok(None) => println!("❌ FAIL - Closed"),
                Err(_) => println!("⏱️  TIMEOUT"),
            }
        }
        Ok(Some(Ok(_))) => println!("✅ PASS"),
        Ok(Some(Err(_))) => println!("❌ FAIL - Error"),
        Ok(None) => println!("❌ FAIL - Closed"),
        Err(_) => println!("⏱️  TIMEOUT"),
    }

    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

/// Test a private channel
async fn test_private<F>(
    name: &str,
    ctx: &ExampleContext,
    _market: lighter_client::types::MarketId,
    _account: lighter_client::types::AccountId,
    configure: F,
) -> Result<()>
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

    // Wait for first message (Connected or data)
    match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
        Ok(Some(Ok(WsEvent::Connected))) => {
            // Got connected, wait for data
            match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
                Ok(Some(Ok(WsEvent::Unknown(raw)))) => {
                    if raw.contains("\"error\"") {
                        println!("❌ FAIL - Error");
                    } else {
                        println!("✅ PASS ({} bytes)", raw.len());
                    }
                }
                Ok(Some(Ok(_))) => println!("✅ PASS"),
                Ok(Some(Err(_))) => println!("❌ FAIL - Error"),
                Ok(None) => println!("❌ FAIL - Closed"),
                Err(_) => println!("⏱️  TIMEOUT"),
            }
        }
        Ok(Some(Ok(_))) => println!("✅ PASS"),
        Ok(Some(Err(_))) => println!("❌ FAIL - Error"),
        Ok(None) => println!("❌ FAIL - Closed"),
        Err(_) => println!("⏱️  TIMEOUT"),
    }

    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}
