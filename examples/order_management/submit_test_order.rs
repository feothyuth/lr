//! Submit a REAL test order to MAINNET
//!
//! Order Details:
//! - Market: 0 (BTC-USDC or similar)
//! - Size: 0.005 (very small test size)
//! - Price: 3500 (far below market, won't fill)
//! - Type: Limit, Post-Only
//! - Side: BUY
//!
//! Required environment variables:
//! - `LIGHTER_PRIVATE_KEY`
//! - `LIGHTER_ACCOUNT_INDEX` or `ACCOUNT_INDEX` or `LIGHTER_ACCOUNT_ID`
//! - `LIGHTER_API_KEY_INDEX` or `API_KEY_INDEX`
//!
//! Optional configuration:
//! - Create a `config.toml` (or set `LIGHTER_CONFIG_PATH`) to override defaults:
//!   ```toml
//!   [defaults]
//!   market_id = 0
//!   order_size = 100
//!   price_ticks = 110000
//!
//!   [submit_test_order]
//!   price_ticks = 105000
//!   ```
//!
//! Run with: cargo run --example submit_test_order

#[path = "../common/config.rs"]
mod common_config;
#[path = "../common/env.rs"]
mod common_env;

use common_config::get_i64 as config_i64;
use common_env::{ensure_positive, parse_env, require_env, require_parse_env, resolve_api_url};
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, BaseQty, Expiry, MarketId, Price},
};
use time::Duration;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Submitting REAL test order to MAINNET");
    println!("⚠️  THIS IS A REAL TRADE WITH REAL FUNDS");
    println!();

    // Order parameters
    let market_id: i32 = parse_env::<i32>(&["LIGHTER_MARKET_ID"])
        .or_else(|| {
            config_i64(&["submit_test_order.market_id", "defaults.market_id"])
                .map(|value| i32::try_from(value).expect("market_id must fit in i32"))
        })
        .unwrap_or(0);
    // Size: defaults to 100 (0.001 base lots) unless overridden
    let size = parse_env::<i64>(&["LIGHTER_ORDER_QTY"])
        .or_else(|| config_i64(&["submit_test_order.order_size", "defaults.order_size"]))
        .unwrap_or(100);
    // Price: Use config or a reasonable default near market
    let price_ticks = parse_env::<i64>(&["LIGHTER_LIMIT_TICKS"])
        .or_else(|| config_i64(&["submit_test_order.price_ticks", "defaults.price_ticks"]))
        .unwrap_or(110_000); // Slightly below current market ~115,000

    println!("📝 Order Details:");
    println!("   Market ID: {}", market_id);
    println!("   Size (raw): {}", size);
    println!("   Price (ticks): {}", price_ticks);
    println!("   Type: Limit, Post-Only");
    println!("   Side: BUY");
    println!("   Expires: 10 minutes (minimum allowed)");
    println!();
    println!(
        "⚠️  Note: Price {} is below current market (~115,000)",
        price_ticks
    );
    println!("   Post-only order will sit on the order book.");
    println!();

    // Build client
    println!("🔐 Building client...");
    let client = build_client().await?;
    println!("   ✅ Client created");
    println!();

    // Create and submit the order
    println!("✍️  Creating order...");
    let market = MarketId::new(market_id);
    let qty = BaseQty::try_from(size).expect("size must be non-zero");

    println!("📡 Preparing to submit order to blockchain...");
    println!("   Press Ctrl+C now if you want to cancel!");
    println!("   Submitting in 3 seconds...");
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Submit order with proper error handling
    match client
        .order(market)
        .buy()
        .qty(qty)
        .limit(Price::ticks(price_ticks))
        .expires_at(Expiry::from_now(Duration::minutes(10)))
        .post_only()
        .submit()
        .await
    {
        Ok(submission) => {
            println!("   ✅ Order submitted successfully!");
            println!();
            println!("📊 Result:");
            println!("   TX Hash: {}", submission.response().tx_hash);
            println!();
            println!("✅ Order placed! Check your account:");
            println!("   Visit: https://app.lighter.xyz");
            println!();
            println!("⚠️  Order details:");
            println!("   - This is a BUY order at {} ticks", price_ticks);
            println!("   - Current market price is ~115,000 ticks");
            println!(
                "   - Your order is {} ticks below market",
                115_000 - price_ticks
            );
            println!("   - You can cancel it anytime");
            Ok(())
        }
        Err(e) => {
            // Check if this is an OrderRejected error
            if let Some(_err_str) = format!("{:?}", e).strip_prefix("Signer(OrderRejected") {
                eprintln!();
                eprintln!("❌ Order was rejected by the exchange!");
                eprintln!();
                eprintln!("Error details:");
                eprintln!("{}", e);
                eprintln!();
                eprintln!("Common reasons for rejection:");
                eprintln!("   1. Expiry too short (must be ≥10 minutes)");
                eprintln!("   2. Price outside fat-finger bounds (95%-105% of mark price)");
                eprintln!("   3. Post-only order would cross the spread");
                eprintln!("   4. Insufficient margin");
                eprintln!("   5. Order size below minimum");
                Err(e.into())
            } else {
                // Other errors (network, validation, etc.)
                eprintln!("❌ Failed to submit order: {}", e);
                Err(e.into())
            }
        }
    }
}

async fn build_client() -> Result<LighterClient, Box<dyn std::error::Error>> {
    let api_url = resolve_api_url("https://mainnet.zklighter.elliot.ai");
    let private_key = require_env(&["LIGHTER_PRIVATE_KEY"], "LIGHTER_PRIVATE_KEY");
    let account_index = ensure_positive(
        require_parse_env::<i64>(
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
        require_parse_env::<i32>(&["LIGHTER_API_KEY_INDEX", "API_KEY_INDEX"], "API key index");

    println!("   API URL: {}", api_url);
    println!("   Account Index: {}", account_index);
    println!("   API Key Index: {}", api_key_index);

    Ok(LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?)
}
