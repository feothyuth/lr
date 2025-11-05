// Send a SAFE Test Order via REST API using lighter_client SDK
// Demonstrates the complete flow: get market price, place safe order
//
// SAFETY: Order is placed 20% away from market with post-only, so it WON'T FILL

use anyhow::Result;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, BaseQty, Expiry, MarketId, Price},
};
use time::Duration;

#[path = "../common/env.rs"]
mod env;

use env::resolve_api_url;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("📝 Send SAFE Test Order via REST API (using lighter_client SDK)");
    println!("{}\n", "═".repeat(80));

    // Load environment
    dotenvy::dotenv().ok();

    let private_key = std::env::var("LIGHTER_PRIVATE_KEY").expect("LIGHTER_PRIVATE_KEY not set");
    let account_index: i64 = std::env::var("LIGHTER_ACCOUNT_INDEX")
        .or_else(|_| std::env::var("ACCOUNT_INDEX"))
        .unwrap_or_else(|_| "0".to_string())
        .parse()?;
    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")
        .or_else(|_| std::env::var("API_KEY_INDEX"))
        .unwrap_or_else(|_| "0".to_string())
        .parse()?;

    println!("🔧 Configuration:");
    println!("   Account: {}", account_index);
    println!("   API Key: {}", api_key_index);
    println!();

    // Initialize SDK client
    println!("{}", "─".repeat(80));
    println!("Step 1: Initialize SDK Client");
    println!("{}", "─".repeat(80));

    let client = LighterClient::builder()
        .api_url(resolve_api_url("https://mainnet.zklighter.elliot.ai"))
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    println!("   ✅ Client initialized\n");

    // Step 2: Get market price
    println!("{}", "─".repeat(80));
    println!("📈 Step 2: Get Market Price");
    println!("{}", "─".repeat(80));

    // Use SDK to get market data
    let market_id = MarketId::new(0);
    let orderbook = client.orders().book_details(Some(market_id)).await?;
    let order_book_detail = &orderbook.order_book_details[0];

    let last_price = order_book_detail.last_trade_price;

    println!("   Last trade price: ${:.2}\n", last_price);

    // Step 3: Create safe order (20% below market, won't fill)
    println!("{}", "─".repeat(80));
    println!("🛡️  Step 3: Create Safe Test Order");
    println!("{}", "─".repeat(80));

    let safe_price = last_price * 0.80; // 20% below market
    let size: f64 = 0.01; // 0.01 ETH = ~$40 at $4000

    // Convert to scaled integers (price in cents, size in 0.0001 ETH)
    let price_scaled = (safe_price * 100.0).round() as i64;
    let size_scaled = (size * 10000.0).round() as i64;

    println!("   ⚠️  SAFETY:");
    println!(
        "   - Order is 20% BELOW market (${:.2} vs ${:.2})",
        safe_price, last_price
    );
    println!("   - Post-only flag = order won't cross spread");
    println!("   - Small size = 0.01 ETH (~${:.0})", size * last_price);
    println!("   - Order will sit on book, not fill");
    println!();

    println!("   📝 Order params:");
    println!("   - Market: 0 (ETH-PERP)");
    println!("   - Side: BUY");
    println!("   - Size: {} ETH", size);
    println!("   - Price: ${:.2}", safe_price);
    println!("   - Type: Limit Post-Only");
    println!();

    // Step 4: Submit order using SDK
    println!("{}", "─".repeat(80));
    println!("🚀 Step 4: Submit Order via SDK");
    println!("{}", "─".repeat(80));

    let submission = client
        .order(market_id)
        .buy()
        .qty(BaseQty::try_from(size_scaled).map_err(|e| anyhow::anyhow!("{}", e))?)
        .limit(Price::ticks(price_scaled))
        .expires_at(Expiry::from_now(Duration::minutes(15)))
        .post_only()
        .submit()
        .await?;

    println!("   ✅ Order submitted!\n");

    // Parse response
    println!("   Response code: {}", submission.response().code);

    if !submission.response().tx_hash.is_empty() {
        println!("   Transaction hash: {}", submission.response().tx_hash);
    }

    if let Some(message) = &submission.response().message {
        if !message.is_empty() {
            println!("   Message: {}", message);
        }
    }

    println!("\n{}", "═".repeat(80));
    println!("✅ Transaction sent successfully!");
    println!("{}", "═".repeat(80));
    println!("\n💡 Note: This was a SAFE test order:");
    println!("   - Placed far from market (won't fill)");
    println!("   - Post-only (won't cross spread)");
    println!("   - SDK handles all signing and submission");
    println!("   - No manual nonce or form-encoding needed");
    println!();

    Ok(())
}
