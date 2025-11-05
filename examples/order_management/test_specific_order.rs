//! Test transaction submission with specific price and size
//!
//! User requested test:
//! - Price: $100,000
//! - Size: 0.001 BTC
//!
//! Run with:
//! LIGHTER_PRIVATE_KEY="..." ACCOUNT_INDEX="..." LIGHTER_API_KEY_INDEX="0" \
//! cargo run --example test_specific_order

use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, BaseQty, Expiry, MarketId, Price},
    ws_client::WsEvent,
};
use time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   SPECIFIC ORDER TEST - LIGHTER DEX MAINNET                  ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();
    println!("📋 Test Parameters:");
    println!("   Price: $100,000");
    println!("   Size: 0.001 BTC");
    println!();

    // Load configuration
    let private_key = std::env::var("LIGHTER_PRIVATE_KEY").expect("LIGHTER_PRIVATE_KEY not set");
    let account_index: i64 = std::env::var("ACCOUNT_INDEX")
        .expect("ACCOUNT_INDEX not set")
        .parse()?;
    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")
        .unwrap_or_else(|_| "0".to_string())
        .parse()?;

    println!("📋 Configuration:");
    println!("   Account Index: {}", account_index);
    println!("   API Key Index: {}", api_key_index);
    println!("   Market: BTC-PERP (ID: 1)");
    println!();

    // Build client
    let api_url = "https://mainnet.zklighter.elliot.ai";
    let client = LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    println!("✅ Client initialized");
    println!();

    let market = MarketId::new(1); // BTC-PERP

    // Get current market price for comparison
    println!("📡 Fetching current market price...");
    let mut stream = client.ws().subscribe_market_stats(market).connect().await?;

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

    let mark_price = mark_price.ok_or("Failed to get mark price from WebSocket")?;
    println!("✅ Current mark price: ${}", mark_price);
    println!();

    // Calculate order parameters
    // BTC has price_decimals = 1, so multiply by 10
    let target_price = 100_000.0;
    let price_ticks = (target_price * 10.0) as i64; // 100,000 * 10 = 1,000,000 ticks

    // BTC has size_decimals = 5
    // 0.001 BTC = 0.001 * 10^5 = 100 units
    let target_size = 0.001;
    let size_units = (target_size * 100_000.0) as i64; // 100 units

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📊 ORDER DETAILS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("Market Context:");
    println!("   Current mark price: ${}", mark_price);
    println!("   Target price: ${}", target_price);
    println!(
        "   Difference: ${} ({:.2}%)",
        mark_price - target_price,
        ((mark_price - target_price) / mark_price * 100.0)
    );
    println!();
    println!("Order Parameters:");
    println!("   Type: Limit, Post-Only");
    println!("   Side: BUY");
    println!("   Price: {} ticks (${:.2})", price_ticks, target_price);
    println!("   Size: {} units ({} BTC)", size_units, target_size);
    println!("   Expiry: 10 minutes");
    println!();

    // Calculate if this is within acceptable range
    let price_diff_pct = ((mark_price - target_price) / mark_price * 100.0).abs();
    if price_diff_pct > 5.0 {
        println!(
            "⚠️  WARNING: Price is {:.2}% away from mark price",
            price_diff_pct
        );
        println!("   Lighter typically rejects orders >5% from mark (fat-finger protection)");
        println!("   This order will likely be rejected.");
        println!();
    }

    println!("⏰ Submitting order in 3 seconds... (Ctrl+C to cancel)");
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    println!();

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📡 SUBMITTING ORDER");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client
        .order(market)
        .buy()
        .qty(BaseQty::try_from(size_units)?)
        .limit(Price::ticks(price_ticks))
        .expires_at(Expiry::from_now(Duration::minutes(10)))
        .post_only()
        .submit()
        .await
    {
        Ok(submission) => {
            println!("✅ ORDER SUBMITTED SUCCESSFULLY!");
            println!();
            println!("📊 Result:");
            println!("   TX Hash: {}", submission.response().tx_hash);
            println!();
            println!("📋 Order Details:");
            println!("   Price: ${} ({} ticks)", target_price, price_ticks);
            println!("   Size: {} BTC ({} units)", target_size, size_units);
            println!("   Distance from mark: {:.2}% below", price_diff_pct);
            println!();
            println!("✅ Check your order at: https://app.lighter.xyz");
        }
        Err(e) => {
            println!("❌ ORDER REJECTED");
            println!();
            println!("Error: {}", e);
            println!();

            // Parse error message
            let error_str = format!("{}", e);
            if error_str.contains("21734") || error_str.contains("too far from the mark price") {
                println!("💡 Explanation:");
                println!(
                    "   The exchange rejected this order because the price (${}) is too",
                    target_price
                );
                println!("   far from the current mark price (${})", mark_price);
                println!();
                println!(
                    "   Difference: {:.2}% (Lighter typically allows <5%)",
                    price_diff_pct
                );
                println!();
                println!("   This is fat-finger protection to prevent accidental orders.");
                println!();
                println!("📋 To place an order at this price:");
                println!("   1. Wait for market to move closer to $100,000");
                println!(
                    "   2. Use a price closer to current market (e.g., ${:.0})",
                    mark_price * 0.97
                );
                println!("   3. Submit as a series of orders, moving price gradually");
            } else if error_str.contains("21733") || error_str.contains("accidental price") {
                println!("💡 Explanation:");
                println!("   Order flagged as potential fat-finger error");
            } else {
                println!("💡 This might be a different issue. Check the error details above.");
            }
        }
    }

    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("✅ TEST COMPLETE");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    Ok(())
}
