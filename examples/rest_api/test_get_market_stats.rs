// Get Market Statistics via REST API
// Now using the official lighter_client SDK
// PUBLIC endpoint - No authentication required

use anyhow::Result;
use lighter_client::LighterClient;

#[path = "../common/env.rs"]
mod env;

use env::resolve_api_url;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("📈 Get Exchange Statistics (REST API - Public)");
    println!("{}\n", "═".repeat(80));

    // Create client using the official SDK
    let client = LighterClient::new(resolve_api_url("https://mainnet.zklighter.elliot.ai")).await?;

    println!("🔍 Fetching exchange-wide statistics...");

    let response = client.orders().exchange_stats().await?;

    println!("\n✅ Response received!\n");

    // Display exchange-wide totals
    println!("{}", "═".repeat(80));
    println!("Exchange-Wide Summary");
    println!("{}", "═".repeat(80));
    println!("  24h USD Volume:  ${:.2}", response.daily_usd_volume);
    println!("  24h Trades:      {}", response.daily_trades_count);
    println!("  Total Markets:   {}", response.order_book_stats.len());

    // Display per-market statistics
    println!("\n{}", "═".repeat(80));
    println!("Market Statistics");
    println!("{}", "═".repeat(80));

    for market_stat in &response.order_book_stats {
        println!("\n{}", "─".repeat(80));
        println!("🔹 Market: {}", market_stat.symbol);
        println!("{}", "─".repeat(80));

        // Price info
        println!(
            "  💵 Last Trade Price:     ${:.2}",
            market_stat.last_trade_price
        );

        // 24h statistics
        println!(
            "  📊 24h Volume (Base):    {:.4}",
            market_stat.daily_base_token_volume
        );
        println!(
            "  💰 24h Volume (Quote):   ${:.2}",
            market_stat.daily_quote_token_volume
        );
        println!(
            "  📈 24h Price Change:     {:.2}%",
            market_stat.daily_price_change
        );
        println!(
            "  🔢 24h Trades:           {}",
            market_stat.daily_trades_count
        );
    }

    println!("\n{}", "═".repeat(80));
    println!("✅ Exchange statistics fetched successfully!");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
