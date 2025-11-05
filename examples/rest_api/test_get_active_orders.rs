// Get Active Orders via lighter_client SDK
// PRIVATE endpoint - Authentication required
//
// Returns all currently active (open) orders for the account including:
// - Order ID and client order ID
// - Market ID and symbol
// - Price, quantity, side (buy/sell)
// - Order type (limit, market, post-only, etc.)
// - Status and timestamps

use anyhow::Result;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, MarketId},
};

#[path = "../common/env.rs"]
mod env;

use env::resolve_api_url;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("📋 Get Active Orders (lighter_client SDK)");
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

    // Market ID (0 = ETH-PERP, 1 = BTC-PERP, etc.)
    let market_id = std::env::var("LIGHTER_MARKET_ID")
        .or_else(|_| std::env::var("MARKET_ID"))
        .unwrap_or_else(|_| "0".to_string())
        .parse::<i32>()?;

    println!("📝 Account Index: {}", account_index);
    println!("🔑 API Key Index: {}", api_key_index);
    println!("📈 Market ID: {}", market_id);

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

    println!("📊 Fetching active orders for market {}...", market_id);

    // Use SDK's typed method to get active orders
    let orders_response = client
        .account()
        .active_orders(MarketId::new(market_id))
        .await?;

    println!("\n✅ Active orders received!\n");

    println!("{}", "─".repeat(80));
    println!("📋 Active Orders");
    println!("{}", "─".repeat(80));

    if orders_response.orders.is_empty() {
        println!("ℹ️  No active orders found");
    } else {
        println!("📊 Total Active Orders: {}\n", orders_response.orders.len());

        for (i, order) in orders_response.orders.iter().enumerate() {
            println!("{}", "─".repeat(80));
            println!("Order #{}", i + 1);
            println!("{}", "─".repeat(80));

            // Order identification
            println!("🆔 Order Index: {}", order.order_index);
            println!("🏷️  Client Order ID: {}", order.client_order_id);

            // Market info
            println!("📈 Market ID: {}", order.market_index);

            // Order details
            let side_emoji = if order.is_ask { "🔴" } else { "🟢" };
            let side_str = if order.is_ask { "SELL" } else { "BUY" };
            println!("{} Side: {}", side_emoji, side_str);
            println!("💵 Price: {}", order.price);
            println!("📦 Quantity: {}", order.initial_base_amount);
            println!("✅ Filled: {}", order.filled_base_amount);

            // Order type
            println!("🔖 Type: {:?}", order.r#type);

            // Timestamps
            println!("🕐 Block Height: {}", order.block_height);
            println!("🕐 Timestamp: {}", order.timestamp);

            println!();
        }
    }

    // Show full response for debugging
    println!("{}", "─".repeat(80));
    println!("📄 Full JSON Response:");
    println!("{}", "─".repeat(80));
    let json_str = serde_json::to_string_pretty(&orders_response)?;
    println!("{}", &json_str[..std::cmp::min(1000, json_str.len())]);
    if json_str.len() > 1000 {
        println!(
            "\n... (truncated, {} more characters)",
            json_str.len() - 1000
        );
    }

    println!("\n{}", "═".repeat(80));
    println!("✅ Active orders fetched successfully!");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
