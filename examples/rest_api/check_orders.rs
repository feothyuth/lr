//! Check open orders for an account
//!
//! Run with: cargo run --example check_orders

#[path = "../common/env.rs"]
mod env;

use env::resolve_api_url;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, MarketId},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Checking open orders...");
    println!();

    let private_key =
        std::env::var("LIGHTER_PRIVATE_KEY").expect("LIGHTER_PRIVATE_KEY must be set");

    let account_index: i64 = std::env::var("ACCOUNT_INDEX")
        .or_else(|_| std::env::var("LIGHTER_ACCOUNT_INDEX"))
        .expect("ACCOUNT_INDEX must be set")
        .parse()
        .expect("Account index must be a number");

    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")
        .unwrap_or_else(|_| "0".to_string())
        .parse()
        .expect("API key index must be a number");

    let api_url = resolve_api_url("https://mainnet.zklighter.elliot.ai");

    println!("📝 Configuration:");
    println!("   API URL: {}", api_url);
    println!("   Account Index: {}", account_index);
    println!("   API Key Index: {}", api_key_index);
    println!();

    let client = LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    println!("✅ Client created");
    println!();

    // Check market 0 for active orders
    println!("📋 Checking active orders on market 0...");
    let market_0 = MarketId::new(0);

    match client.account().active_orders(market_0).await {
        Ok(orders) => {
            if orders.orders.is_empty() {
                println!("   ❌ No active orders found on market 0");
                println!();
                println!("Possible reasons:");
                println!("   1. Order expired (minimum expiry is 10 minutes)");
                println!("   2. Order rejected (expiry was below 10 min minimum)");
                println!("   3. Order filled immediately");
                println!("   4. Size/price format incorrect");
                println!("   5. Order on wrong market");
            } else {
                println!(
                    "   ✅ Found {} active order(s) on market 0:",
                    orders.orders.len()
                );
                println!();
                for (i, order) in orders.orders.iter().enumerate() {
                    println!("   Order {}:", i + 1);
                    println!("      Market ID: {}", order.market_index);
                    println!("      Order ID: {}", order.order_index);
                    println!("      Side: {}", if order.is_ask { "SELL" } else { "BUY" });
                    println!("      Price: {}", order.price);
                    println!("      Size (remaining): {}", order.remaining_base_amount);
                    println!("      Size (initial): {}", order.initial_base_amount);
                    println!();
                }
            }
        }
        Err(e) => {
            println!("   ⚠️  Error fetching active orders: {}", e);
        }
    }

    println!();

    // Also check market 1 just in case
    println!("📋 Checking active orders on market 1 (just in case)...");
    let market_1 = MarketId::new(1);

    match client.account().active_orders(market_1).await {
        Ok(orders) => {
            if !orders.orders.is_empty() {
                println!(
                    "   ✅ Found {} active order(s) on market 1:",
                    orders.orders.len()
                );
                println!("   (You requested market 0, but order might be here)");
                println!();
                for (i, order) in orders.orders.iter().enumerate() {
                    println!("   Order {}:", i + 1);
                    println!("      Market ID: {}", order.market_index);
                    println!("      Order ID: {}", order.order_index);
                    println!("      Side: {}", if order.is_ask { "SELL" } else { "BUY" });
                    println!("      Price: {}", order.price);
                    println!("      Size: {}", order.remaining_base_amount);
                    println!();
                }
            } else {
                println!("   No orders on market 1 either");
            }
        }
        Err(e) => {
            println!("   ⚠️  Error: {}", e);
        }
    }

    Ok(())
}
