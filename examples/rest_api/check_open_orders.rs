#[path = "../common/env.rs"]
mod common_env;

use common_env::{ensure_positive, parse_env_or, require_env, require_parse_env, resolve_api_url};
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, MarketId},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let private_key = require_env(&["LIGHTER_PRIVATE_KEY"], "LIGHTER_PRIVATE_KEY");
    let account_index = ensure_positive(
        require_parse_env(
            &[
                "LIGHTER_ACCOUNT_INDEX",
                "ACCOUNT_INDEX",
                "LIGHTER_ACCOUNT_ID",
            ],
            "account index",
        ),
        "account index",
    );
    let api_key_index =
        require_parse_env(&["LIGHTER_API_KEY_INDEX", "API_KEY_INDEX"], "API key index");
    let market_index = parse_env_or(&["MARKET_INDEX", "LIGHTER_MARKET_ID"], 0);

    let client = LighterClient::builder()
        .api_url(resolve_api_url("https://mainnet.zklighter.elliot.ai"))
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   CHECKING OPEN ORDERS ON LIGHTER DEX                        ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    // Get active orders for the selected market
    println!("📊 Fetching active orders for market {market_index}...");
    let active_orders = client
        .account()
        .active_orders(MarketId::new(market_index))
        .await?;

    println!("📋 Account Index: {}", account_index);
    println!();

    let order_count = active_orders.orders.len();

    if order_count == 0 {
        println!("═══════════════════════════════════════════════════════════════");
        println!("⚠️  NO ACTIVE ORDERS ON THE BOOK");
        println!("═══════════════════════════════════════════════════════════════");
        println!();
        println!("Possible reasons from stress test:");
        println!("   • Limit orders were 5% away from market price");
        println!("   • Exchange rejected orders as too far from mark");
        println!("   • Orders expired (10 minute expiry)");
        println!("   • Market orders filled and closed immediately");
        println!();
        println!("💡 To see visible orders, submit closer to market:");
        println!("   • Within 1-2% of current mark price");
        println!("   • Post-only to ensure they rest on book");
        println!("   • Longer expiry time");
    } else {
        println!("═══════════════════════════════════════════════════════════════");
        println!("✅ ACTIVE ORDERS ON BOOK: {}", order_count);
        println!("═══════════════════════════════════════════════════════════════");
        println!();

        for (i, order) in active_orders.orders.iter().enumerate() {
            println!("Order #{}:", i + 1);
            println!("   Market ID: {}", order.market_index);
            println!("   Side: {}", if order.is_ask { "SELL" } else { "BUY " });
            println!("   Price: {}", order.price);
            println!("   Initial Size: {}", order.initial_base_amount);
            println!("   Remaining Size: {}", order.remaining_base_amount);
            println!("   Filled: {}", order.filled_base_amount);
            println!("   Status: {:?}", order.status);
            println!("   Trigger Status: {:?}", order.trigger_status);
            println!("   Timestamp: {}", order.timestamp);
            println!();
        }
    }

    // Check positions
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("POSITIONS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    let details = client.account().details().await?;
    if let Some(account) = details.accounts.first() {
        let has_positions = account
            .positions
            .iter()
            .any(|p| p.position.parse::<f64>().unwrap_or(0.0) != 0.0);

        if !has_positions {
            println!("   No open positions");
        } else {
            for pos in &account.positions {
                let size: f64 = pos.position.parse().unwrap_or(0.0);
                if size != 0.0 {
                    println!(
                        "   Market {}: {} BTC (sign: {})",
                        pos.market_id,
                        size,
                        if pos.sign == 1 { "LONG" } else { "SHORT" }
                    );
                }
            }
        }
    }
    println!();

    Ok(())
}
