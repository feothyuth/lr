//! Test what format the payloads are in

use anyhow::{Context, Result};
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, BaseQty, MarketId, Nonce, Price},
};

#[tokio::main]
async fn main() -> Result<()> {
    println!("🔍 Testing Payload Formats");
    println!("===========================\n");

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

    // Build client
    let client = LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key)
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .account_index(AccountId::new(account_index))
        .build()
        .await
        .context("Failed to build LighterClient")?;

    let signer = client
        .signer()
        .context("Client not configured with signer")?;

    // Test 1: Cancel All Orders payload
    println!("📋 Test 1: Cancel All Orders Payload");
    println!("-------------------------------------");

    let (cancel_api_key, cancel_nonce) = signer.next_nonce().await?;
    let cancel_payload = signer
        .sign_cancel_all_orders(0, 0, Some(cancel_nonce), Some(cancel_api_key))
        .await?;

    println!("Raw payload length: {} bytes", cancel_payload.len());
    println!(
        "First 200 chars: {}",
        &cancel_payload[..cancel_payload.len().min(200)]
    );

    // Try to parse as JSON
    match serde_json::from_str::<serde_json::Value>(&cancel_payload) {
        Ok(json) => {
            println!("✅ Valid JSON!");
            println!("JSON structure: {}", serde_json::to_string_pretty(&json)?);
        }
        Err(e) => {
            println!("❌ NOT valid JSON: {}", e);
        }
    }

    // Test 2: Order payload from order builder
    println!("\n📋 Test 2: Order Builder Payload");
    println!("---------------------------------");

    let market = MarketId::new(0); // ETH market
    let price = Price::ticks(300000); // $3000.00
    let qty = BaseQty::try_from(1000).map_err(|e| anyhow::anyhow!("Invalid qty: {}", e))?; // 0.1 ETH

    let (order_api_key, order_nonce) = signer.next_nonce().await?;
    let order_signed = client
        .order(market)
        .buy()
        .qty(qty)
        .limit(price)
        .post_only()
        .with_api_key(ApiKeyIndex::new(order_api_key))
        .with_nonce(Nonce::new(order_nonce))
        .sign()
        .await?;

    let order_payload = order_signed.payload();
    println!("Raw payload length: {} bytes", order_payload.len());
    println!(
        "First 200 chars: {}",
        &order_payload[..order_payload.len().min(200)]
    );

    // Try to parse as JSON
    match serde_json::from_str::<serde_json::Value>(order_payload) {
        Ok(json) => {
            println!("✅ Valid JSON!");
            println!("JSON structure: {}", serde_json::to_string_pretty(&json)?);
        }
        Err(e) => {
            println!("❌ NOT valid JSON: {}", e);
        }
    }

    println!("\n✅ Test complete!");

    Ok(())
}
