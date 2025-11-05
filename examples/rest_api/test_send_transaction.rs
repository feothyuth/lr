// Send Transaction via REST API using lighter_client SDK
// Demonstrates: Using SDK's OrderBuilder to create and submit orders
//
// Required environment variables:
// - LIGHTER_PRIVATE_KEY
// - LIGHTER_ACCOUNT_INDEX (or ACCOUNT_INDEX)
// - LIGHTER_API_KEY_INDEX (or API_KEY_INDEX)

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
    println!("📝 Send Transaction via REST API (using lighter_client SDK)");
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

    println!("📊 Account: {}", account_index);
    println!("🔑 API Key: {}", api_key_index);
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

    // Create safe order (20% below market)
    println!("{}", "─".repeat(80));
    println!("Step 2: Create and Submit Order (20% below market - won't fill)");
    println!("{}", "─".repeat(80));

    let market_id = 0; // ETH market
    let size = 100; // 0.01 ETH (100 = 0.01 * 10000)
    let price = 300000; // $3000 (300000 = 3000 * 100)

    println!("   Market: {} (ETH-PERP)", market_id);
    println!("   Side: BUY");
    println!("   Size: 0.01 ETH");
    println!("   Price: $3000 (20% below market)");
    println!("   Type: Limit Post-Only");
    println!();

    // Submit order using SDK
    let submission = client
        .order(MarketId::new(market_id))
        .buy()
        .qty(BaseQty::try_from(size).map_err(|e| anyhow::anyhow!("{}", e))?)
        .limit(Price::ticks(price))
        .expires_at(Expiry::from_now(Duration::minutes(15)))
        .post_only()
        .submit()
        .await?;

    println!("   ✅ Order submitted!\n");

    // Show response
    println!("{}", "─".repeat(80));
    println!("Response:");
    println!("{}", "─".repeat(80));
    println!("   Code: {}", submission.response().code);

    if !submission.response().tx_hash.is_empty() {
        println!("   Transaction Hash: {}", submission.response().tx_hash);
    }

    if let Some(msg) = &submission.response().message {
        if !msg.is_empty() {
            println!("   Message: {}", msg);
        }
    }

    println!("\n{}", "═".repeat(80));
    println!("✅ Transaction sent via SDK!");
    println!("{}", "═".repeat(80));
    println!("\n💡 Key Points:");
    println!("   - SDK handles all signing and submission internally");
    println!("   - No need to manage nonces manually (SDK handles it)");
    println!("   - No need to construct form-urlencoded requests");
    println!("   - Simple, type-safe OrderBuilder API");
    println!();

    Ok(())
}
