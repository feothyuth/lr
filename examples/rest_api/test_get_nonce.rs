// Get Next Nonce via lighter_client SDK
// PRIVATE endpoint - Authentication required
//
// This endpoint returns the next nonce value needed for signing transactions.
// It's a utility function you'll need before creating orders.

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
    println!("🔢 Get Next Nonce (lighter_client SDK)");
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

    println!(
        "📊 Fetching next nonce for account {} (API key {})...",
        account_index, api_key_index
    );

    // Use SDK's typed method to get next nonce
    let nonce_response = client
        .account()
        .next_nonce(ApiKeyIndex::new(api_key_index))
        .await?;

    println!("\n✅ Nonce response received!\n");

    // Display nonce
    println!("🔢 Next Nonce: {}", nonce_response.nonce);

    // Show full response
    println!("\n{}", "─".repeat(80));
    println!("📋 Full Response:");
    println!("{}", "─".repeat(80));
    println!("{}", serde_json::to_string_pretty(&nonce_response)?);

    println!("\n{}", "═".repeat(80));
    println!("✅ Nonce fetched successfully!");
    println!("\n💡 Use this nonce value when creating your next transaction.");
    println!("   Note: The SDK typically manages nonces automatically.");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
