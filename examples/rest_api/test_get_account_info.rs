// Get Account Information via lighter_client SDK
// PRIVATE endpoint - Authentication required
//
// Returns comprehensive account information including:
// - Account address and index
// - Margin balances (initial, maintenance, available)
// - Leverage and tier information
// - Account status and configuration
// - Positions data

use anyhow::Result;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex},
};

#[path = "../common/env.rs"]
mod env;

use env::resolve_api_url;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("👤 Get Account Information (lighter_client SDK)");
    println!("{}\n", "═".repeat(80));

    // Load environment variables
    dotenvy::dotenv().ok();

    let account_index = std::env::var("LIGHTER_ACCOUNT_INDEX")
        .or_else(|_| std::env::var("ACCOUNT_INDEX"))
        .unwrap_or_else(|_| "0".to_string())
        .parse::<i64>()?;

    let api_key_index = std::env::var("LIGHTER_API_KEY_INDEX")
        .or_else(|_| std::env::var("API_KEY_INDEX"))
        .unwrap_or_else(|_| "0".to_string())
        .parse::<i32>()?;

    let private_key =
        std::env::var("LIGHTER_PRIVATE_KEY").expect("LIGHTER_PRIVATE_KEY must be set in .env file");

    println!("📝 Account Index: {}", account_index);
    println!("🔑 API Key Index: {}", api_key_index);
    println!(
        "🔑 Private Key: {}...{}",
        &private_key[..6.min(private_key.len())],
        &private_key[private_key.len().saturating_sub(4)..]
    );

    println!("\n🔄 Creating authenticated client...");

    // Create authenticated client using lighter_client SDK
    let client = LighterClient::builder()
        .api_url(resolve_api_url("https://mainnet.zklighter.elliot.ai"))
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    println!("✅ Client created successfully!\n");

    println!("📊 Fetching account details...");

    // Use SDK's typed method to get account details
    let details = client.account().details().await?;

    println!("\n✅ Account details received!\n");

    println!("{}", "─".repeat(80));
    println!("📋 Account Summary");
    println!("{}", "─".repeat(80));

    // Access the first account in the response
    if let Some(account) = details.accounts.first() {
        // Basic account info
        println!("🔢 Account Index: {}", account.account_index);
        println!("🏦 L1 Address: {}", account.l1_address);
        println!("📛 Name: {}", account.name);
        println!("📝 Description: {}", account.description);

        println!("\n{}", "─".repeat(80));
        println!("💰 Account Balances");
        println!("{}", "─".repeat(80));

        // Balance information
        println!("💵 Available Balance: ${}", account.available_balance);
        println!("💎 Collateral: ${}", account.collateral);
        println!("📊 Total Asset Value: ${}", account.total_asset_value);
        println!("🔗 Cross Asset Value: ${}", account.cross_asset_value);

        // Account status
        println!("\n{}", "─".repeat(80));
        println!("📊 Account Status");
        println!("{}", "─".repeat(80));
        println!("🔖 Account Type: {}", account.account_type);
        println!("✅ Status: {}", account.status);

        // Position information
        let non_zero_positions: Vec<_> = account
            .positions
            .iter()
            .filter(|p| p.position_value.parse::<f64>().unwrap_or(0.0) > 0.0)
            .collect();

        println!("\n{}", "─".repeat(80));
        println!("📈 Position Summary");
        println!("{}", "─".repeat(80));
        println!("Open Positions: {}", non_zero_positions.len());
        println!("Total Orders: {}", account.total_order_count);

        // Show full response
        println!("\n{}", "─".repeat(80));
        println!("📄 Full JSON Response:");
        println!("{}", "─".repeat(80));
        println!("{}", serde_json::to_string_pretty(&details)?);
    } else {
        println!("⚠️  No account details returned. Check your credentials.");
    }

    println!("\n{}", "═".repeat(80));
    println!("✅ Account information fetched successfully!");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
