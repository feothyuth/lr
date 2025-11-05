// Get Recent Trades via REST API
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
    println!("📊 Get Recent Trades (REST API - Public)");
    println!("{}\n", "═".repeat(80));

    // Create client using the official SDK
    let client = LighterClient::new(resolve_api_url("https://mainnet.zklighter.elliot.ai")).await?;

    // Test with ETH-PERP (market_id = 0)
    let market_id = 0;
    let limit = 10;

    println!(
        "📈 Fetching {} recent trades for market {}...",
        limit, market_id
    );

    let response = client
        .orders()
        .recent(MarketId::new(market_id), limit)
        .await?;

    println!("\n✅ Response received!\n");

    // Parse recent trades data from SDK response
    let trades = &response.trades;
    if !trades.is_empty() {
        println!("{}", "─".repeat(80));
        println!("Recent Trades:");
        println!("{}", "─".repeat(80));
        println!(
            "{:<20} {:<12} {:<12} {:<12} {:<10}",
            "Time", "Price", "Size", "USD Amount", "Side"
        );
        println!("{}", "─".repeat(80));

        for trade in trades.iter().take(limit as usize) {
            let price = &trade.price;
            let size = &trade.size;
            let usd_amount = &trade.usd_amount;
            let timestamp = trade.timestamp;

            // Determine side from is_maker_ask
            let side_colored = if trade.is_maker_ask {
                "🟢 BUY"
            } else {
                "🔴 SELL"
            };

            // Format timestamp (Unix timestamp to human-readable)
            let time_str = if timestamp > 0 {
                let datetime = chrono::DateTime::from_timestamp(timestamp / 1000, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| "Invalid".to_string());
                datetime
            } else {
                "N/A".to_string()
            };

            println!(
                "{:<20} ${:<11} {:<12} ${:<11} {}",
                time_str, price, size, usd_amount, side_colored
            );
        }

        println!("{}", "─".repeat(80));

        // Calculate some basic statistics
        let total_trades = trades.len();
        let buy_count = trades.iter().filter(|t| t.is_maker_ask).count();
        let sell_count = trades.iter().filter(|t| !t.is_maker_ask).count();

        // Calculate volume
        let total_volume: f64 = trades
            .iter()
            .filter_map(|t| t.size.parse::<f64>().ok())
            .sum();

        let total_usd_volume: f64 = trades
            .iter()
            .filter_map(|t| t.usd_amount.parse::<f64>().ok())
            .sum();

        println!("\n📊 Trade Statistics:");
        println!("  Total Trades:     {}", total_trades);
        println!(
            "  Buy Trades:       {} ({:.1}%)",
            buy_count,
            (buy_count as f64 / total_trades as f64) * 100.0
        );
        println!(
            "  Sell Trades:      {} ({:.1}%)",
            sell_count,
            (sell_count as f64 / total_trades as f64) * 100.0
        );
        println!("  Total Volume:     {:.4} tokens", total_volume);
        println!("  Total USD Volume: ${:.2}", total_usd_volume);

        // Price range
        let prices: Vec<f64> = trades
            .iter()
            .filter_map(|t| t.price.parse::<f64>().ok())
            .collect();

        if !prices.is_empty() {
            let max_price = prices.iter().cloned().fold(f64::MIN, f64::max);
            let min_price = prices.iter().cloned().fold(f64::MAX, f64::min);
            println!("  Price Range:      ${:.2} - ${:.2}", min_price, max_price);
        }
    } else {
        println!("⚠️  No trades found");
    }

    println!("\n{}", "═".repeat(80));
    println!("✅ Recent trades fetched successfully!");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
