//! Demonstrates submitting a transaction via WebSocket.
//!
//! This example shows how to:
//! 1. Connect to WebSocket with authentication
//! 2. Sign a create order transaction
//! 3. Submit the transaction via WebSocket (faster than REST API)
//! 4. Wait for transaction response
//!
//! Required environment variables:
//! - `LIGHTER_PRIVATE_KEY` - Your private key
//! - `LIGHTER_API_URL` (optional) - defaults to mainnet
//! - `LIGHTER_MARKET_ID` (optional) - defaults to 0 (ETH)
//! - `LIGHTER_ACCOUNT_INDEX` (optional) - defaults to 0

#[path = "../common/env.rs"]
mod common_env;

use common_env::{ensure_positive, parse_env_or, require_env, require_parse_env, resolve_api_url};
use lighter_client::{
    lighter_client::LighterClient,
    signer_client::SignerClient,
    tx_executor::{send_tx_ws, TX_TYPE_CREATE_ORDER},
    types::MarketId,
};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load configuration
    let private_key = require_env(&["LIGHTER_PRIVATE_KEY"], "LIGHTER_PRIVATE_KEY");
    let api_url = resolve_api_url("https://mainnet.zklighter.elliot.ai");
    let market_id: i32 = parse_env_or(&["LIGHTER_MARKET_ID"], 0);
    let account_index = ensure_positive(
        require_parse_env(
            &[
                "LIGHTER_ACCOUNT_INDEX",
                "ACCOUNT_INDEX",
                "LIGHTER_ACCOUNT_ID",
            ],
            "account index",
        ),
        "account index",
    );
    let api_key_index =
        require_parse_env(&["LIGHTER_API_KEY_INDEX", "API_KEY_INDEX"], "API key index");

    println!("🚀 Lighter DEX WebSocket Transaction Submission");
    println!("📊 Market ID: {}", market_id);
    println!("👤 Account Index: {}", account_index);

    // Initialize SignerClient for transaction signing
    println!("\n🔐 Initializing signer...");
    let signer = SignerClient::new(
        &api_url,
        &private_key,
        api_key_index,
        account_index,
        None,
        None,
        lighter_client::nonce_manager::NonceManagerType::Optimistic,
        None,
    )
    .await?;

    // Create authentication token
    println!("🎟️  Creating authentication token...");
    let auth_token = signer.create_auth_token_with_expiry(None)?;
    let preview: String = auth_token.token.chars().take(20).collect();
    println!(
        "✅ Auth token created: {}{}",
        preview,
        if auth_token.token.len() > 20 {
            "…"
        } else {
            ""
        }
    );

    // Connect to WebSocket
    println!("\n🔗 Connecting to WebSocket...");
    let client = LighterClient::new(&api_url).await?;
    let market = MarketId::new(market_id);

    // Create WebSocket stream with order book subscription
    let mut stream = client.ws().subscribe_order_book(market).connect().await?;

    // Set authentication token for transaction submission
    stream.connection_mut().set_auth_token(auth_token.token);

    println!("✅ WebSocket connected");

    // Get current nonce
    let (api_key_index, nonce) = signer.next_nonce().await?;
    println!("\n📝 Signing order transaction...");
    println!("   API Key Index: {}", api_key_index);
    println!("   Nonce: {}", nonce);

    // Sign a limit order (example: buy 0.001 ETH at $3000)
    let signed_order = signer
        .sign_create_order(
            market_id,                              // market_index (0 = ETH)
            chrono::Utc::now().timestamp_millis(),  // client_order_index
            10,                                     // base_amount (0.001 ETH with 4 decimals)
            300000,                                 // price (3000.00 with 2 decimals)
            false,                                  // is_ask (false = BUY)
            signer.order_type_limit(),              // order_type (LIMIT)
            signer.order_time_in_force_post_only(), // time_in_force (POST_ONLY)
            false,                                  // reduce_only
            0,                                      // trigger_price (0 = no trigger)
            -1,                                     // order_expiry (-1 = default 28 days)
            Some(nonce),
            Some(api_key_index),
        )
        .await?;

    println!("✅ Order signed successfully");

    // Submit transaction via WebSocket
    println!("\n📤 Submitting transaction via WebSocket...");
    let success = send_tx_ws(
        stream.connection_mut(),
        TX_TYPE_CREATE_ORDER,
        signed_order.payload(),
    )
    .await?;

    if success {
        println!("✅ Transaction submitted successfully!");
        println!(
            "   Client Order Index: {}",
            chrono::Utc::now().timestamp_millis()
        );
        println!("   Side: BUY");
        println!("   Size: 0.001 ETH");
        println!("   Price: $3000.00");
    } else {
        println!("❌ Transaction failed or was rejected by exchange");
        println!("   This is expected if you don't have sufficient balance");
    }

    // Wait a bit for any additional responses
    println!("\n⏳ Waiting for additional WebSocket events...");
    tokio::time::sleep(Duration::from_secs(2)).await;

    println!("\n👋 Example completed!");
    Ok(())
}
