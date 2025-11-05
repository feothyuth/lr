// Get Account P&L via lighter_client SDK
// PRIVATE endpoint - Requires authentication
//
// Returns profit & loss information including:
//   - Realized P&L (from closed positions)
//   - Unrealized P&L (from open positions)
//   - Total P&L
//   - Time-series data at specified resolution

use anyhow::Result;
use lighter_client::{
    lighter_client::{LighterClient, PnlResolution, TimeRange, Timestamp},
    types::{AccountId, ApiKeyIndex},
};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "../common/env.rs"]
mod env;

use env::resolve_api_url;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("💰 Get Account P&L (lighter_client SDK)");
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

    println!("📝 Account Index: {}", account_index);
    println!("🔑 API Key Index: {}", api_key_index);

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

    println!("🔄 Fetching P&L data...\n");

    // Calculate time range (7 days ago to now)
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let seven_days_ago = now - (7 * 24 * 60 * 60); // 7 days in seconds

    // Use SDK's typed method to get P&L
    // Note: API expects "1d" format, not "day", so we use Custom variant
    let pnl_response = client
        .account()
        .pnl(
            PnlResolution::Custom("1d".into()),
            TimeRange::new(Timestamp::new(seven_days_ago)?, Timestamp::new(now)?)?,
            7,           // count_back: last 7 days
            Some(false), // don't ignore transfers
        )
        .await?;

    println!("✅ P&L data received!\n");

    // Display overall P&L
    println!("{}", "─".repeat(80));
    println!("💰 P&L Summary:");
    println!("{}", "─".repeat(80));
    println!();

    if let Some(pnl) = pnl_response.pnl.first() {
        // PnLEntry has: trade_pnl, inflow, outflow, pool_pnl, pool_inflow, pool_outflow
        let trade_pnl = pnl.trade_pnl;
        let inflow = pnl.inflow;
        let outflow = pnl.outflow;
        let net_flow = inflow - outflow;
        let total = trade_pnl + net_flow;

        let trade_pnl_icon = if trade_pnl >= 0.0 { "🟢" } else { "🔴" };
        let net_flow_icon = if net_flow >= 0.0 { "🟢" } else { "🔴" };
        let total_icon = if total >= 0.0 { "🟢" } else { "🔴" };

        println!("  Trade P&L:       {} ${:>12.2}", trade_pnl_icon, trade_pnl);
        println!("  Inflow:          🟢 ${:>12.2}", inflow);
        println!("  Outflow:         🔴 ${:>12.2}", outflow);
        println!("  Net Flow:        {} ${:>12.2}", net_flow_icon, net_flow);
        println!("  ─────────────────────────────────");
        println!("  Total P&L:       {} ${:>12.2}", total_icon, total);
        println!();

        // Show timestamp
        println!("  Timestamp:       {}", pnl.timestamp);
    } else {
        println!("  No P&L data available");
    }

    // Display time-series P&L if available
    if pnl_response.pnl.len() > 1 {
        println!("{}", "─".repeat(80));
        println!("📊 P&L Time Series (Last {} Days):", pnl_response.pnl.len());
        println!("{}", "─".repeat(80));
        println!();

        for (i, pnl_entry) in pnl_response.pnl.iter().enumerate() {
            let trade_pnl = pnl_entry.trade_pnl;
            let net_flow = pnl_entry.inflow - pnl_entry.outflow;
            let total = trade_pnl + net_flow;
            let total_icon = if total >= 0.0 { "🟢" } else { "🔴" };

            print!("  Day {}: {} ${:>10.2}", i + 1, total_icon, total);
            print!(" (ts: {})", pnl_entry.timestamp);
            println!();
        }
        println!();
    }

    println!("{}", "═".repeat(80));
    println!("✅ P&L fetched successfully!");
    println!("{}\n", "═".repeat(80));

    // Print raw JSON for debugging
    println!("🔍 Raw JSON Response:");
    println!("{}", serde_json::to_string_pretty(&pnl_response)?);
    println!();

    Ok(())
}
