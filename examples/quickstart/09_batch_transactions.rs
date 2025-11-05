//! Demonstrates submitting multiple transactions in a single batch via WebSocket.
//!
//! This example shows how to:
//! 1. Sign multiple transactions
//! 2. Submit them as a batch via WebSocket (max 50 per batch)
//! 3. Receive individual success/failure results
//!
//! Batch transactions are useful for:
//! - Placing multiple orders at once (e.g., grid trading)
//! - Canceling multiple orders simultaneously
//! - Updating multiple positions
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
    tx_executor::{send_batch_tx_ws, TX_TYPE_CREATE_ORDER},
    types::MarketId,
};

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

    println!("🚀 Lighter DEX Batch Transaction Submission");
    println!("📊 Market ID: {}", market_id);
    println!("👤 Account Index: {}", account_index);
    println!();
    println!("💡 This example creates a 5-level grid of limit orders");
    println!("   Each order will be submitted in a single batch via WebSocket");

    // Initialize SignerClient
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
    println!("✅ Auth token created");

    // Connect to WebSocket
    println!("\n🔗 Connecting to WebSocket...");
    let client = LighterClient::new(&api_url).await?;
    let market = MarketId::new(market_id);

    // Create WebSocket stream with order book subscription
    let mut stream = client.ws().subscribe_order_book(market).connect().await?;

    // Set authentication token for transaction submission
    stream.connection_mut().set_auth_token(auth_token.token);

    println!("✅ WebSocket connected");

    // Create grid of 5 buy orders
    println!("\n📝 Signing 5 limit orders for batch submission...");
    let base_price = 3000_00; // $3000.00 with 2 decimals
    let price_increment = 10_00; // $10.00 spacing
    let order_size = 10; // 0.001 ETH with 4 decimals

    let mut batch_txs = Vec::new();

    for i in 0..5 {
        let (api_key_index, nonce) = signer.next_nonce().await?;
        let price = base_price - (i * price_increment); // Descending prices for buy orders

        let signed_order = signer
            .sign_create_order(
                market_id,
                chrono::Utc::now().timestamp_millis() + i as i64, // Unique client order index
                order_size,
                price,
                false, // is_ask (false = BUY)
                signer.order_type_limit(),
                signer.order_time_in_force_post_only(),
                false,
                0,
                -1,
                Some(nonce),
                Some(api_key_index),
            )
            .await?;

        println!(
            "   ✅ Order {} signed: BUY 0.001 ETH @ ${}.{}",
            i + 1,
            price / 100,
            price % 100
        );

        batch_txs.push((TX_TYPE_CREATE_ORDER, signed_order.payload().to_string()));
    }

    // Submit batch via WebSocket
    println!(
        "\n📤 Submitting batch of {} transactions via WebSocket...",
        batch_txs.len()
    );
    println!("   Max batch size: 50 transactions per batch");
    println!("   Current batch: {} transactions", batch_txs.len());

    let results = send_batch_tx_ws(stream.connection_mut(), batch_txs).await?;

    // Display results
    println!("\n📊 Batch Submission Results:");
    let success_count = results.iter().filter(|&&r| r).count();
    let fail_count = results.len() - success_count;

    for (i, success) in results.iter().enumerate() {
        let status = if *success {
            "✅ SUCCESS"
        } else {
            "❌ FAILED"
        };
        println!("   Order {}: {}", i + 1, status);
    }

    println!();
    println!("📈 Summary:");
    println!("   Total orders: {}", results.len());
    println!("   Successful: {}", success_count);
    println!("   Failed: {}", fail_count);

    if fail_count > 0 {
        println!();
        println!("💡 Failed orders may be due to:");
        println!("   - Insufficient balance");
        println!("   - Invalid price levels");
        println!("   - Post-only orders crossing the spread");
        println!("   - Rate limiting");
    }

    // Wait for additional WebSocket events
    println!("\n⏳ Waiting for order confirmations...");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    println!("\n👋 Example completed!");
    println!("💡 Batch transactions are ideal for:");
    println!("   - Grid trading strategies");
    println!("   - Market making with multiple levels");
    println!("   - Mass order cancellations");
    println!("   - Portfolio rebalancing");

    Ok(())
}
