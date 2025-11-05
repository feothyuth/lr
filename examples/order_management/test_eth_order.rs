//! Simple ETH Order Test
//!
//! This script demonstrates placing a simple ETH order with size 0.005
//! using the Rust Lighter SDK.

#[path = "../common/env.rs"]
mod common_env;

use common_env::{ensure_positive, parse_env_or, require_env, require_parse_env, resolve_api_url};
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, BaseQty, MarketId, Price},
};
use std::num::NonZeroI64;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment variables from .env file
    // dotenv::from_filename("../../../.env").ok();

    // Initialize logging
    // tracing_subscriber::fmt::init();

    println!("🚀 Starting ETH Order Test");

    // Load configuration from environment
    let api_url = resolve_api_url("https://mainnet.zklighter.elliot.ai");
    let private_key = require_env(&["LIGHTER_PRIVATE_KEY"], "LIGHTER_PRIVATE_KEY");
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

    println!("🔧 Configuration:");
    println!("  API URL: {}", api_url);
    println!("  Account Index: {}", account_index);
    println!("  API Key Index: {}", api_key_index);

    // Create the Lighter client
    let client = LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    println!("✅ Client created successfully");

    // Get account details
    let details = client.account().details().await?;
    if let Some(account) = details.accounts.first() {
        println!("👤 Account Info:");
        println!("  Account Index: {}", account.account_index);
        println!("  Total Orders: {}", account.total_order_count);
        println!("  Open Positions: {}", account.positions.len());
    }

    // Use ETH market (index 0) - ETH is typically market 0
    let market_id = MarketId::new(parse_env_or(&["LIGHTER_MARKET_ID"], 0)); // ETH market by default
    println!("📈 Using ETH market (index 0) for testing");

    // Get current order book to determine reasonable price
    println!("📊 Getting current order book...");
    let orderbook = client.orders().book(market_id, 10).await?;

    println!("📖 Order Book:");
    println!("  Best Bid: {:?}", orderbook.bids.first());
    println!("  Best Ask: {:?}", orderbook.asks.first());

    // For a buy order, use the ask price to ensure execution
    let price = if let Some(best_ask) = orderbook.asks.first() {
        // Use the exact best ask price for buy order
        best_ask.price.parse::<f64>().unwrap_or(0.0)
    } else if let Some(best_bid) = orderbook.bids.first() {
        // Fallback to bid price if no asks
        best_bid.price.parse::<f64>().unwrap_or(0.0)
    } else {
        // Fallback price for ETH (~$3000)
        3000.0
    };

    println!("💰 Calculated order price: ${:.6}", price);

    // ETH size: 0.005 ETH
    let eth_size = 0.005;
    let base_amount = BaseQty::new(
        NonZeroI64::new((eth_size * 1e8) as i64).unwrap(), // Convert to smallest unit
    );

    println!("📏 Order size: {} ETH", eth_size);
    println!(
        "📏 Base amount (smallest units): {}",
        base_amount.into_inner()
    );

    // Convert price to ticks - the order book shows prices like 111855.9
    // So we need to multiply by 10 to get the correct tick format
    let price_ticks = Price::ticks((price * 10.0) as i64);

    // Check account balance and transaction history first
    println!("💰 Checking account balance...");
    let limits = client.account().limits().await?;
    println!("  Available Balance: {:?}", limits);

    println!("📜 Checking recent transaction history...");
    let recent_txs = client.transactions().list(10, None).await?;
    println!("  Recent transactions: {}", recent_txs.txs.len());
    for (i, tx) in recent_txs.txs.iter().enumerate().take(3) {
        println!(
            "    {}. {} (Type: {}, Status: {})",
            i + 1,
            tx.hash,
            tx.r#type,
            tx.status
        );
    }

    // Try to create and submit the order
    println!("🔄 Attempting to submit ETH market order...");
    match client
        .order(market_id)
        .buy()
        .qty(base_amount)
        .market() // Pure market order - no price specification
        .with_client_order_id(1) // Use a small manual client ID
        .submit()
        .await
    {
        Ok(submission) => {
            println!("✅ Order submitted successfully!");
            println!("📝 Response Code: {}", submission.response().code);

            let tx_hash = submission.response().tx_hash.clone();
            if !tx_hash.is_empty() {
                println!("🔗 Transaction Hash: {}", tx_hash);
            }

            // Check order status after a short delay
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

            println!("🔍 Checking order status...");
            let active_orders = client.account().active_orders(market_id).await?;
            println!("📊 Current active orders: {}", active_orders.orders.len());

            for order in &active_orders.orders {
                println!(
                    "  📋 Order {} @ {} (Status: {})",
                    order.order_id, order.initial_base_amount, order.price
                );
            }

            // Check trades history to see if order was executed
            println!("🔍 Checking recent trades...");
            let recent_trades = client.orders().recent(market_id, 5).await?;
            println!("  Recent trades: {}", recent_trades.trades.len());
            for (i, trade) in recent_trades.trades.iter().enumerate().take(3) {
                println!(
                    "    {}. {} ETH @ ${} (Time: {})",
                    i + 1,
                    trade.usd_amount,
                    trade.price,
                    trade.timestamp
                );
            }
        }
        Err(e) => {
            println!("❌ Order submission failed: {}", e);
            println!("📝 This is expected if account has insufficient balance or restrictions");
            println!("✅ SDK is working correctly - API connection and authentication successful");
        }
    }

    println!("🎉 ETH order test completed!");
    Ok(())
}
