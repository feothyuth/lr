//! Place Visible Orders Close to Market
//!
//! Places 5 BUY and 5 SELL orders within 1% of mark price
//! so they're actually visible on the order book
//!
//! Run with:
//! LIGHTER_PRIVATE_KEY="..." ACCOUNT_INDEX="..." LIGHTER_API_KEY_INDEX="0" \
//! cargo run --example place_visible_orders

#[path = "../common/example_context.rs"]
mod common;

use futures_util::StreamExt;
use lighter_client::{
    types::{BaseQty, Expiry, Price},
    ws_client::WsEvent,
};
use time::Duration;

use common::ExampleContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   PLACING VISIBLE ORDERS CLOSE TO MARKET                     ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    let ctx = ExampleContext::initialise(Some("place_visible_orders")).await?;
    let client = ctx.client();
    let account_index = ctx.account_id().into_inner();
    let api_key_index = ctx.api_key_index().into_inner();
    let market = ctx.market_id();
    let market_index: i32 = market.into();

    println!("✅ Client initialized");
    println!("📋 Configuration:");
    println!("   Account Index: {}", account_index);
    println!("   API Key Index: {}", api_key_index);
    println!("   Market ID: {}", market_index);
    println!();

    // Get current market price
    println!("📡 Fetching current market price...");
    let mut stream = ctx
        .ws_builder()
        .subscribe_market_stats(market)
        .connect()
        .await?;

    let mut mark_price: Option<f64> = None;
    let timeout = tokio::time::Duration::from_secs(10);
    let start = tokio::time::Instant::now();

    while start.elapsed() < timeout && mark_price.is_none() {
        tokio::select! {
            Some(event) = stream.next() => {
                if let Ok(WsEvent::MarketStats(stats)) = event {
                    mark_price = Some(stats.market_stats.mark_price.parse::<f64>()?);
                    break;
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {}
        }
    }

    let mark_price = mark_price.ok_or("Failed to get mark price")?;
    println!("✅ Current mark price: ${:.2}", mark_price);
    println!();

    let size = 20; // 0.0002 BTC (minimum)

    // Place 5 BUY orders 0.5-1.5% below market
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Placing 5 BUY orders (0.5-1.5% below market)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    for i in 1..=5 {
        let pct_below = 0.005 + (i as f64 * 0.002); // 0.5%, 0.7%, 0.9%, 1.1%, 1.3%
        let price = ((mark_price * (1.0 - pct_below)) * 10.0) as i64;
        let price_dollars = price as f64 / 10.0;

        match client
            .order(market)
            .buy()
            .qty(BaseQty::try_from(size)?)
            .limit(Price::ticks(price))
            .expires_at(Expiry::from_now(Duration::hours(1)))
            .post_only()
            .submit()
            .await
        {
            Ok(submission) => {
                println!(
                    "   ✅ BUY #{}: ${:.2} ({:.2}% below market)",
                    i,
                    price_dollars,
                    pct_below * 100.0
                );
                println!("      TX: {}", submission.response().tx_hash);
            }
            Err(e) => {
                println!("   ❌ BUY #{}: FAILED - {}", i, e);
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    println!();

    // Place 5 SELL orders 0.5-1.5% above market
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Placing 5 SELL orders (0.5-1.5% above market)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    for i in 1..=5 {
        let pct_above = 0.005 + (i as f64 * 0.002); // 0.5%, 0.7%, 0.9%, 1.1%, 1.3%
        let price = ((mark_price * (1.0 + pct_above)) * 10.0) as i64;
        let price_dollars = price as f64 / 10.0;

        match client
            .order(market)
            .sell()
            .qty(BaseQty::try_from(size)?)
            .limit(Price::ticks(price))
            .expires_at(Expiry::from_now(Duration::hours(1)))
            .post_only()
            .submit()
            .await
        {
            Ok(submission) => {
                println!(
                    "   ✅ SELL #{}: ${:.2} ({:.2}% above market)",
                    i,
                    price_dollars,
                    pct_above * 100.0
                );
                println!("      TX: {}", submission.response().tx_hash);
            }
            Err(e) => {
                println!("   ❌ SELL #{}: FAILED - {}", i, e);
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("✅ ORDERS PLACED");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("These orders should now be visible on the order book!");
    println!("Check your account at: https://app.lighter.xyz");
    println!();

    Ok(())
}
