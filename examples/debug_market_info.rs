//! Debug script to check market configuration and minimum order requirements

use anyhow::{Context, Result};
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex},
};

#[tokio::main]
async fn main() -> Result<()> {
    println!("🔍 Checking Lighter Market Configuration");
    println!("=========================================\n");

    // Get environment variables
    let private_key =
        std::env::var("LIGHTER_PRIVATE_KEY").context("LIGHTER_PRIVATE_KEY not set")?;
    let account_index: i64 = std::env::var("LIGHTER_ACCOUNT_INDEX")
        .or_else(|_| std::env::var("ACCOUNT_INDEX"))
        .context("LIGHTER_ACCOUNT_INDEX or ACCOUNT_INDEX not set")?
        .parse()
        .context("Failed to parse account index")?;
    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")
        .unwrap_or_else(|_| "0".to_string())
        .parse()
        .context("Failed to parse LIGHTER_API_KEY_INDEX")?;

    let api_url = std::env::var("LIGHTER_API_URL")
        .unwrap_or_else(|_| "https://mainnet.zklighter.elliot.ai".to_string());

    println!("🔧 Configuration:");
    println!("   API URL: {}", api_url);
    println!("   Account Index: {}", account_index);
    println!("   API Key Index: {}", api_key_index);

    // Build client
    let client = LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key)
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .account_index(AccountId::new(account_index))
        .build()
        .await
        .context("Failed to build LighterClient")?;

    println!("\n📊 Fetching market metadata...\n");

    // Get all market details
    let metadata = client
        .orders()
        .book_details(None) // Get all markets
        .await
        .context("Failed to fetch market metadata")?;

    for detail in metadata.order_book_details.iter() {
        println!("Market ID: {}", detail.market_id);
        println!("  Price Decimals: {}", detail.price_decimals);
        println!("  Size Decimals: {}", detail.size_decimals);

        let price_multiplier: i64 = 10_i64
            .checked_pow(detail.price_decimals as u32)
            .unwrap_or(0);
        let size_multiplier: i64 = 10_i64.checked_pow(detail.size_decimals as u32).unwrap_or(0);

        println!("  Price Multiplier: {}", price_multiplier);
        println!("  Size Multiplier: {}", size_multiplier);
        println!("  Tick Size: {:.8}", 1.0 / price_multiplier as f64);
        println!("  Lot Size: {:.8}", 1.0 / size_multiplier as f64);

        // Check if this is market 1 (the one used in simple_twoside_mm.rs)
        if detail.market_id == 1 {
            println!("\n⚠️  Market 1 Analysis (used in simple_twoside_mm.rs):");
            let order_size = 0.0002; // From the script
            let scaled_size = order_size * size_multiplier as f64;
            println!("  ORDER_SIZE (0.0002) * size_multiplier = {}", scaled_size);
            println!("  This translates to {} lots", scaled_size);

            // Estimate minimum order size
            let min_lots = 100; // Common minimum
            let min_size = min_lots as f64 / size_multiplier as f64;
            println!("  Estimated minimum order size: {} (100 lots)", min_size);

            if order_size < min_size {
                println!("  ❌ ORDER_SIZE is likely TOO SMALL!");
                println!("  💡 Try using ORDER_SIZE = {} or larger", min_size);
            }
        }

        println!("---");
    }

    // Also check current positions
    println!("\n📈 Checking account state...");

    // Try to get positions (this API might not exist in the current SDK version)
    println!("Note: Position and order APIs may vary by SDK version");

    println!("\n✅ Debug complete!");

    Ok(())
}
