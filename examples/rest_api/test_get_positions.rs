// Get Account Positions via lighter_client SDK
// PRIVATE endpoint - Requires authentication
//
// NOTE: Positions are part of the account details response
//
// ⚠️ CRITICAL: Position Data Structure Gotcha!
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Lighter returns position data with TWO separate fields:
//   1. `sign`: -1 for SHORT, 1 for LONG (direction indicator)
//   2. `position`: ALWAYS POSITIVE (unsigned absolute value)
//
// ❌ WRONG: if position > 0 { is_long = true }  // BUG: position is ALWAYS positive!
// ✅ CORRECT: if sign > 0 { is_long = true }    // Use 'sign' field!
//
// To get signed position: signed_position = position * sign
//
// To close a position:
//   - LONG (sign=1): Place SELL order (is_ask=1)
//   - SHORT (sign=-1): Place BUY order (is_ask=0)

use anyhow::Result;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex},
};

#[path = "../common/env.rs"]
mod env;

use env::resolve_api_url;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("💼 Get Account Positions (lighter_client SDK)");
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

    println!("🔄 Fetching positions for account {}...\n", account_index);

    // Use SDK's account details to get positions
    let details = client.account().details().await?;

    println!("✅ Account details received!\n");

    // Access the first account in the response
    let account = details
        .accounts
        .first()
        .ok_or_else(|| anyhow::anyhow!("No account found in response"))?;

    // Filter for non-zero positions
    let positions: Vec<_> = account
        .positions
        .iter()
        .filter(|p| p.position_value.parse::<f64>().unwrap_or(0.0) > 0.0)
        .collect();

    if positions.is_empty() {
        println!("ℹ️  No open positions found.");
        println!("\n{}", "═".repeat(80));
        println!("✅ Positions fetched successfully (empty)");
        println!("{}\n", "═".repeat(80));
        return Ok(());
    }

    println!("{}", "─".repeat(80));
    println!("📈 Open Positions ({}):", positions.len());
    println!("{}", "─".repeat(80));
    println!();

    for (i, pos) in positions.iter().enumerate() {
        let position_value: f64 = pos.position_value.parse().unwrap_or(0.0);
        let sign: i32 = pos.sign; // sign is already i32, not String
        let signed_pos = position_value * sign as f64;
        let direction = if sign > 0 { "LONG 🟢" } else { "SHORT 🔴" };
        let close_side = if sign > 0 { "SELL" } else { "BUY" };

        println!("Position #{}", i + 1);
        println!("  Market ID:      {}", pos.market_id);
        println!("  Symbol:         {}", pos.symbol);
        println!();
        println!("  ⚠️ Position Fields (see WARNING at top of this file!):");
        println!(
            "    - sign:       {} (direction: {})",
            sign,
            if sign > 0 { "LONG" } else { "SHORT" }
        );
        println!(
            "    - position:   {} (ALWAYS POSITIVE!)",
            pos.position_value
        );
        println!();
        println!("  ✅ Computed Values:");
        println!("    - Signed Position:  {:.4}", signed_pos);
        println!("    - Direction:        {}", direction);
        println!("    - Is Long?:         {} (sign > 0)", sign > 0);
        println!("    - Is Short?:        {} (sign < 0)", sign < 0);
        println!(
            "    - Close Order Side: {} (to close this position)",
            close_side
        );
        println!();
        println!("{}", "─".repeat(80));
    }

    // Calculate totals
    let total_long: f64 = positions
        .iter()
        .filter(|p| p.sign > 0)
        .map(|p| p.position_value.parse::<f64>().unwrap_or(0.0))
        .sum();

    let total_short: f64 = positions
        .iter()
        .filter(|p| p.sign < 0)
        .map(|p| p.position_value.parse::<f64>().unwrap_or(0.0))
        .sum();

    let net_position = total_long - total_short;

    println!("\n📊 Position Summary:");
    println!("  Total LONG:   {:.4}", total_long);
    println!("  Total SHORT:  {:.4}", total_short);
    println!("  Net Position: {:.4}", net_position);

    println!("\n{}", "═".repeat(80));
    println!("⚠️  CRITICAL REMINDER:");
    println!("   When implementing position logic, ALWAYS use 'sign' field for direction!");
    println!("   The 'position' field is ALWAYS positive, regardless of long/short.");
    println!("{}", "═".repeat(80));

    println!("\n{}", "═".repeat(80));
    println!("✅ Positions fetched successfully!");
    println!("{}\n", "═".repeat(80));

    // Print raw JSON for debugging
    println!("🔍 Raw JSON Response:");
    println!("{}", serde_json::to_string_pretty(&details)?);
    println!();

    Ok(())
}
