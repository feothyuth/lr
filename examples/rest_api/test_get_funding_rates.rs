// Get Funding Rates via REST API
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
    println!("💹 Get Funding Rates (REST API - Public)");
    println!("{}\n", "═".repeat(80));

    // Create client using the official SDK
    let client = LighterClient::new(resolve_api_url("https://mainnet.zklighter.elliot.ai")).await?;

    // Test with ETH-PERP (market_id = 0)
    let market_id = 0;

    println!("📊 Fetching funding rates...");

    let response = client.funding().rates().await?;

    println!("\n✅ Response received!\n");

    // Parse funding rate data from SDK response
    let funding_rates = &response.funding_rates;
    if !funding_rates.is_empty() {
        println!("{}", "═".repeat(80));
        println!("Current Funding Rates");
        println!("{}", "═".repeat(80));

        // Filter for the requested market_id
        let market_rate = funding_rates.iter().find(|r| r.market_id == market_id);

        if let Some(rate_info) = market_rate {
            println!(
                "\n🔹 Market: {} (ID: {})",
                rate_info.symbol, rate_info.market_id
            );
            println!("{}", "─".repeat(80));

            let rate = rate_info.rate;
            let rate_pct = rate * 100.0;
            let emoji = if rate >= 0.0 { "🟢" } else { "🔴" };

            println!("\n  {} Current Rate:       {:.6}%", emoji, rate_pct);

            // Calculate annualized rate (assuming 8h funding periods)
            let annualized = rate * 3.0 * 365.0 * 100.0;
            println!("  📈 Annualized (APR):  {:.2}%", annualized);

            // Interpretation
            if rate > 0.0 {
                println!("\n  💡 Interpretation: Longs pay shorts");
                println!("     (Bullish sentiment - premium to spot)");
            } else if rate < 0.0 {
                println!("\n  💡 Interpretation: Shorts pay longs");
                println!("     (Bearish sentiment - discount to spot)");
            } else {
                println!("\n  💡 Interpretation: Neutral funding");
            }
        } else {
            println!("⚠️  No funding rate found for market {}", market_id);
        }

        // Display all available funding rates
        println!("\n{}", "═".repeat(80));
        println!("All Markets Funding Rates");
        println!("{}", "═".repeat(80));
        println!(
            "{:<15} {:<12} {:<15} {:<15}",
            "Symbol", "Market ID", "Rate", "Annualized"
        );
        println!("{}", "─".repeat(80));

        for rate_entry in funding_rates.iter() {
            let rate_pct = rate_entry.rate * 100.0;
            let annualized = rate_entry.rate * 3.0 * 365.0 * 100.0;
            let emoji = if rate_entry.rate >= 0.0 {
                "🟢"
            } else {
                "🔴"
            };

            println!(
                "{:<15} {:<12} {} {:>6.4}%     {:>6.2}%",
                rate_entry.symbol, rate_entry.market_id, emoji, rate_pct, annualized
            );
        }

        // Calculate average funding rate
        let avg_funding: f64 =
            funding_rates.iter().map(|r| r.rate).sum::<f64>() / funding_rates.len() as f64;

        println!("\n{}", "─".repeat(80));
        println!("📊 Funding Statistics:");
        println!("  Average Rate:         {:.6}%", avg_funding * 100.0);
        println!(
            "  Average APR:          {:.2}%",
            avg_funding * 3.0 * 365.0 * 100.0
        );
        println!("  Markets:              {}", funding_rates.len());
    } else {
        println!("⚠️  No funding rate data found");
    }

    println!("\n{}", "═".repeat(80));
    println!("✅ Funding rates fetched successfully!");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
