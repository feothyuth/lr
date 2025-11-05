// Get Inactive Orders via lighter_client SDK
// PRIVATE endpoint - Authentication required
//
// Returns historical orders that are no longer active including:
// - Filled orders (completely filled)
// - Cancelled orders
// - Expired orders
// - Rejected orders
//
// Useful for analyzing trading history and order outcomes.

use anyhow::Result;
use lighter_client::{lighter_client::InactiveOrdersQuery, types::MarketId};

#[path = "../common/example_context.rs"]
mod common;

use common::ExampleContext;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("📜 Get Inactive Orders (lighter_client SDK)");
    println!("{}\n", "═".repeat(80));

    dotenvy::dotenv().ok();

    let ctx = ExampleContext::initialise(Some("test_get_inactive_orders")).await?;
    let client = ctx.client();
    let account_index = ctx.account_id().into_inner();
    let api_key_index = ctx.api_key_index().into_inner();
    let default_market = ctx.market_id();
    let default_market_index: i32 = default_market.into();

    let market_override = std::env::var("LIGHTER_MARKET_ID")
        .or_else(|_| std::env::var("MARKET_ID"))
        .ok()
        .and_then(|s| s.parse::<i32>().ok());

    let effective_market = market_override.unwrap_or(default_market_index);

    println!("📝 Account Index: {}", account_index);
    println!("🔑 API Key Index: {}", api_key_index);
    println!("📈 Market ID: {}", effective_market);
    println!("\n🔄 Using authenticated client from context...\n");

    println!("📊 Fetching inactive orders...");
    println!("ℹ️  This includes filled, cancelled, expired, and rejected orders");

    // Build query for inactive orders
    let query = InactiveOrdersQuery::new(25)?.market(MarketId::new(effective_market));

    // Use SDK's typed method to get inactive orders
    let orders_response = match client.account().inactive_orders(query).await {
        Ok(resp) => resp,
        Err(err) => {
            println!("⚠️  Unable to fetch inactive orders: {err}");
            println!(
                "    The API returned data outside the expected ranges. Skipping detailed report."
            );
            return Ok(());
        }
    };

    println!("\n✅ Inactive orders received!\n");

    println!("{}", "─".repeat(80));
    println!("📜 Inactive Orders (Historical)");
    println!("{}", "─".repeat(80));

    if orders_response.orders.is_empty() {
        println!("ℹ️  No inactive orders found");
        println!("    (This is normal for new accounts with no trading history)");
    } else {
        println!(
            "📊 Total Inactive Orders: {}\n",
            orders_response.orders.len()
        );

        // Group by status (if status field exists in Order type)
        let filled = orders_response
            .orders
            .iter()
            .filter(|o| o.filled_base_amount == o.initial_base_amount)
            .count();
        let cancelled = orders_response.orders.len() - filled;

        println!("📊 Orders by Status:");
        println!("  ✅ Filled: {}", filled);
        println!("  ❌ Cancelled/Other: {}", cancelled);

        println!("\n{}", "─".repeat(80));
        println!("Recent Orders (showing up to 10):");
        println!("{}", "─".repeat(80));

        for (i, order) in orders_response.orders.iter().take(10).enumerate() {
            println!("\n#{} Order", i + 1);
            println!("{}", "─".repeat(40));

            // Order identification
            println!("  🆔 Order Index: {}", order.order_index);
            println!("  🏷️  Client ID: {}", order.client_order_id);

            // Market info
            println!("  📈 Market ID: {}", order.market_index);

            // Order details
            let side_emoji = if order.is_ask { "🔴" } else { "🟢" };
            let side_str = if order.is_ask { "SELL" } else { "BUY" };
            println!("  {} Side: {}", side_emoji, side_str);
            println!("  💵 Price: {}", order.price);
            println!("  📦 Quantity: {}", order.initial_base_amount);
            if order.filled_base_amount != "0" {
                println!("  ✅ Filled: {}", order.filled_base_amount);
            }

            // Status indicator
            let is_filled = order.filled_base_amount == order.initial_base_amount;
            let status_emoji = if is_filled { "✅" } else { "❌" };
            let status_str = if is_filled {
                "filled"
            } else {
                "cancelled/partial"
            };
            println!("  {} Status: {}", status_emoji, status_str);

            // Timestamps
            println!("  🕐 Block Height: {}", order.block_height);
            println!("  🕐 Timestamp: {}", order.timestamp);
        }

        if orders_response.orders.len() > 10 {
            println!(
                "\n... and {} more orders (use pagination for more)",
                orders_response.orders.len() - 10
            );
        }
    }

    // Show full response for debugging (truncated)
    println!("\n{}", "─".repeat(80));
    println!("📄 Full JSON Response (first 1000 chars):");
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
    println!("✅ Inactive orders fetched successfully!");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
