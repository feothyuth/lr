// Get Account Trade Fills via lighter_client SDK
// PRIVATE endpoint - Requires authentication
//
// Returns trade fills (executed trades) for the account.
// This uses the `trades` endpoint with account-specific filtering.

use anyhow::Result;
use lighter_client::{
    lighter_client::{LighterClient, SortDir, TradeSort, TradesQuery},
    types::{AccountId, ApiKeyIndex, MarketId},
};

#[path = "../common/env.rs"]
mod env;

use env::resolve_api_url;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("📊 Get Trade Fills (lighter_client SDK)");
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

    let market_id = std::env::var("LIGHTER_MARKET_ID")
        .or_else(|_| std::env::var("MARKET_ID"))
        .ok()
        .and_then(|s| s.parse::<i32>().ok());

    println!("📝 Account Index: {}", account_index);
    println!("🔑 API Key Index: {}", api_key_index);
    if let Some(market) = market_id {
        println!("📈 Market ID: {}", market);
    }

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

    println!("🔄 Fetching trade fills...\n");

    // Build query for trades (fills)
    let mut query = TradesQuery::new(TradeSort::Timestamp, 50)? // Last 50 trades
        .direction(SortDir::Desc); // Most recent first

    if let Some(market) = market_id {
        query = query.market(MarketId::new(market));
    }

    // Use SDK's typed method to get trades (fills)
    let trades_response = client.account().trades(query).await?;

    println!("✅ Trade fills received!\n");

    if trades_response.trades.is_empty() {
        println!("ℹ️  No trade fills found.");
        println!("\n{}", "═".repeat(80));
        println!("✅ Fills fetched successfully (empty)");
        println!("{}\n", "═".repeat(80));
        return Ok(());
    }

    println!("📦 Total trade fills: {}\n", trades_response.trades.len());

    println!("{}", "─".repeat(80));
    println!("📈 Recent Trade Fills:");
    println!("{}", "─".repeat(80));
    println!();

    for (i, fill) in trades_response.trades.iter().enumerate() {
        // Trade struct uses is_maker_ask, not is_ask
        // For determining side from the perspective of the trade, we can use is_maker_ask
        let side_icon = if fill.is_maker_ask { "🔴" } else { "🟢" };
        let side_str = if fill.is_maker_ask { "SELL" } else { "BUY" };

        println!("Fill #{}", i + 1);
        println!("  Trade ID:       {}", fill.trade_id);
        println!("  Ask ID:         {}", fill.ask_id);
        println!("  Bid ID:         {}", fill.bid_id);
        println!("  Market ID:      {}", fill.market_id);
        println!();
        println!("  Side:           {} {}", side_icon, side_str);
        println!("  Price:          {}", fill.price);
        println!("  Size:           {}", fill.size);

        // Calculate notional value
        if let (Ok(price), Ok(size)) = (fill.price.parse::<f64>(), fill.size.parse::<f64>()) {
            println!("  Notional:       ${:.2}", price * size);
        }

        // Trade struct has taker_fee and maker_fee, not just fee
        println!("  Taker Fee:      {}", fill.taker_fee);
        println!("  Maker Fee:      {}", fill.maker_fee);

        println!("  Timestamp:      {}", fill.timestamp);
        println!("  Block:          {}", fill.block_height);

        println!();
        println!("{}", "─".repeat(80));
    }

    // Calculate statistics
    let buy_count = trades_response
        .trades
        .iter()
        .filter(|f| !f.is_maker_ask)
        .count();
    let sell_count = trades_response
        .trades
        .iter()
        .filter(|f| f.is_maker_ask)
        .count();

    let total_volume: f64 = trades_response
        .trades
        .iter()
        .filter_map(|f| {
            let price: f64 = f.price.parse().ok()?;
            let size: f64 = f.size.parse().ok()?;
            Some(price * size)
        })
        .sum();

    println!("\n📊 Fill Statistics:");
    println!("  Total Fills:    {}", trades_response.trades.len());
    println!("  Buy Orders:     {} 🟢", buy_count);
    println!("  Sell Orders:    {} 🔴", sell_count);
    println!("  Total Volume:   ${:.2}", total_volume);

    if !trades_response.trades.is_empty() {
        println!(
            "  Avg Fill Size:  ${:.2}",
            total_volume / trades_response.trades.len() as f64
        );
    }

    println!("\n{}", "═".repeat(80));
    println!("✅ Trade fills fetched successfully!");
    println!("{}\n", "═".repeat(80));

    // Print raw JSON sample (first fill only)
    if !trades_response.trades.is_empty() {
        println!("🔍 Raw JSON Sample (first fill):");
        println!(
            "{}",
            serde_json::to_string_pretty(&trades_response.trades[0])?
        );
        println!();
    }

    Ok(())
}
