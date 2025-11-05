// Get Order Book via REST API
// Now using the official lighter_client SDK
// PUBLIC endpoint - No authentication required

use anyhow::Result;
use lighter_client::types::MarketId;
use lighter_client::LighterClient;

#[path = "../common/env.rs"]
mod env;

use env::resolve_api_url;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("📖 Get Order Book (REST API - Public)");
    println!("{}\n", "═".repeat(80));

    // Create client using the official SDK
    let client = LighterClient::new(resolve_api_url("https://mainnet.zklighter.elliot.ai")).await?;

    // Test with ETH-PERP (market_id = 0)
    let market_id = 0;

    println!("📈 Fetching order book for market {}...", market_id);

    // Fetch order book using SDK - get up to 25 levels
    let order_book = client.orders().book(MarketId::new(market_id), 25).await?;

    println!("\n✅ Order book received!\n");

    // Display bids (top 5)
    println!("📗 Bids (top 5):");
    for (i, order) in order_book.bids.iter().take(5).enumerate() {
        println!(
            "   {} - {} × {}",
            i + 1,
            order.price,
            order.remaining_base_amount
        );
    }

    // Display asks (top 5)
    println!("\n📕 Asks (top 5):");
    for (i, order) in order_book.asks.iter().take(5).enumerate() {
        println!(
            "   {} - {} × {}",
            i + 1,
            order.price,
            order.remaining_base_amount
        );
    }

    // Calculate spread if we have both bid and ask
    if let (Some(best_bid), Some(best_ask)) = (order_book.bids.first(), order_book.asks.first()) {
        println!(
            "\n💰 Best Bid: {} | Best Ask: {}",
            best_bid.price, best_ask.price
        );
    }

    // Also fetch exchange stats for 24h volume
    println!("\n📊 Fetching exchange statistics...");
    let stats = client.orders().exchange_stats().await?;
    println!("   24h USD Volume: ${:.2}", stats.daily_usd_volume);
    println!("   24h Trades:     {}", stats.daily_trades_count);

    println!("\n{}", "═".repeat(80));
    println!("✅ Order book fetched successfully!");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
