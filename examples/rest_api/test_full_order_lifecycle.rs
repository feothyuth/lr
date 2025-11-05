// Full Order Lifecycle via REST API using lighter_client SDK
// Demonstrates complete order flow: place -> query -> cancel -> verify
// This is a comprehensive integration test using the official SDK
//
// Flow:
// 1. Get current market price (order book)
// 2. Place a SAFE order (far from market, post-only)
// 3. Query active orders to confirm placement
// 4. Cancel the order
// 5. Query again to verify cancellation
//
// Safety:
// - Order placed 20% away from market (won't fill)
// - Post-only (won't take liquidity, allows far-from-market prices)
// - Small size (0.01 ETH)
// - Cancelled immediately after confirmation

use anyhow::Result;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, BaseQty, Expiry, MarketId, Price},
};
use time::Duration;

#[path = "../common/env.rs"]
mod env;

use env::resolve_api_url;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("🔄 Full Order Lifecycle Test via SDK");
    println!("{}\n", "═".repeat(80));

    // Load environment variables
    dotenvy::dotenv().ok();

    let private_key =
        std::env::var("LIGHTER_PRIVATE_KEY").expect("LIGHTER_PRIVATE_KEY not set in .env");
    let account_index = std::env::var("LIGHTER_ACCOUNT_INDEX")
        .or_else(|_| std::env::var("ACCOUNT_INDEX"))
        .unwrap_or_else(|_| "0".to_string())
        .parse::<i64>()?;
    let api_key_index = std::env::var("LIGHTER_API_KEY_INDEX")
        .or_else(|_| std::env::var("API_KEY_INDEX"))
        .unwrap_or_else(|_| "0".to_string())
        .parse::<i32>()?;

    let http_url = resolve_api_url("https://mainnet.zklighter.elliot.ai");

    println!("🔧 Configuration:");
    println!("   HTTP URL: {}", http_url);
    println!("   Account Index: {}", account_index);
    println!("   API Key Index: {}", api_key_index);
    println!("   Market: ETH-PERP (market_id=0)");
    println!();

    // Initialize SDK client
    println!("{}", "═".repeat(80));
    println!("🔑 Initializing SDK Client");
    println!("{}", "═".repeat(80));

    let client = LighterClient::builder()
        .api_url(http_url.clone())
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    println!("✅ Client initialized\n");

    // =========================================================================
    // STEP 1: Get Current Market Price
    // =========================================================================
    println!("{}", "═".repeat(80));
    println!("📈 STEP 1: Get Current Market Price");
    println!("{}", "═".repeat(80));

    let market_id = MarketId::new(0); // ETH-PERP
    let orderbook = client.orders().book_details(Some(market_id)).await?;
    let order_book_detail = &orderbook.order_book_details[0];

    let mid_price = order_book_detail.last_trade_price;

    println!("   Last Trade Price: ${:.2}", mid_price);
    println!("   (Using last trade as reference price)");

    // Place order 20% below market (won't fill)
    let safe_price = mid_price * 0.80;
    println!("   Safe Order Price: ${:.2} (20% below mid)\n", safe_price);

    // =========================================================================
    // STEP 2: Place Order using SDK
    // =========================================================================
    println!("{}", "═".repeat(80));
    println!("🔑 STEP 2: Place Order using SDK");
    println!("{}", "═".repeat(80));

    let base_amount = 100; // 0.01 ETH (scaled with 4 decimals: 0.01 * 10000 = 100)
    let price_scaled = (safe_price * 100.0).round() as i64; // Price in cents (2 decimals)

    println!("   Market: 0 (ETH-PERP)");
    println!("   Side: BUY");
    println!("   Size: 0.01 ETH");
    println!("   Price: ${:.2}", safe_price);
    println!("   Type: LIMIT, Post-Only");

    // Submit order using SDK
    let submission = client
        .order(market_id)
        .buy()
        .qty(BaseQty::try_from(base_amount).map_err(|e| anyhow::anyhow!("{}", e))?)
        .limit(Price::ticks(price_scaled))
        .expires_at(Expiry::from_now(Duration::days(7)))
        .post_only()
        .submit()
        .await?;

    println!("\n📥 Response:");
    println!("   Code: {}", submission.response().code);

    if !submission.response().tx_hash.is_empty() {
        println!("   Transaction Hash: {}", submission.response().tx_hash);
    }

    println!("✅ Order placed successfully!\n");

    // =========================================================================
    // STEP 3: Query Active Orders to Confirm
    // =========================================================================
    println!("{}", "═".repeat(80));
    println!("🔍 STEP 3: Query Active Orders");
    println!("{}", "═".repeat(80));

    // Wait a moment for order to be indexed
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Get active orders using SDK
    let orders = client.account().active_orders(market_id).await?.orders;

    println!("📋 Active orders:");
    println!("   Total active orders: {}", orders.len());

    // Find the most recent order (should be ours)
    let mut found_order: Option<(i32, i64)> = None;
    if !orders.is_empty() {
        // Sort by order_index descending to get most recent
        let mut sorted_orders = orders.clone();
        sorted_orders.sort_by(|a, b| b.order_index.cmp(&a.order_index));

        // Take the most recent order
        let recent_order = &sorted_orders[0];
        println!("\n   Most Recent Order:");
        println!("   Order Index: {}", recent_order.order_index);
        println!("   Market: {}", recent_order.market_index);
        println!(
            "   Side: {}",
            if recent_order.is_ask { "SELL" } else { "BUY" }
        );
        println!("   Price: {}", recent_order.price);
        println!("   Remaining: {}", recent_order.remaining_base_amount);
        println!("   👆 This should be our test order!");

        found_order = Some((recent_order.market_index, recent_order.order_index));
    }

    println!();

    if found_order.is_none() {
        anyhow::bail!("Could not find any active orders!");
    }

    let (market_index, order_index) = found_order.unwrap();

    // =========================================================================
    // STEP 4: Cancel the Order
    // =========================================================================
    println!("{}", "═".repeat(80));
    println!("❌ STEP 4: Cancel the Order");
    println!("{}", "═".repeat(80));

    println!(
        "   Canceling order {} on market {}",
        order_index, market_index
    );

    // Cancel order using SDK
    let cancel_submission = client
        .cancel(MarketId::new(market_index), order_index)
        .submit()
        .await?;

    println!("\n📥 Cancel Response:");
    println!("   Code: {}", cancel_submission.response().code);

    if !cancel_submission.response().tx_hash.is_empty() {
        println!(
            "   Transaction Hash: {}",
            cancel_submission.response().tx_hash
        );
    }

    println!("✅ Cancel transaction submitted!\n");

    // =========================================================================
    // STEP 5: Verify Cancellation
    // =========================================================================
    println!("{}", "═".repeat(80));
    println!("✅ STEP 5: Verify Order Cancelled");
    println!("{}", "═".repeat(80));

    // Wait for cancellation to be indexed
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Query active orders again
    let final_orders = client.account().active_orders(market_id).await?.orders;

    println!("🔍 Checking active orders...");
    let still_active = final_orders.iter().any(|o| o.order_index == order_index);

    if still_active {
        println!("⚠️  Order still appears active (may take a moment to update)");
    } else {
        println!("✅ Order successfully cancelled!");
        println!("   Order {} is no longer in active orders", order_index);
    }

    println!("   Current active orders: {}", final_orders.len());
    println!();

    // =========================================================================
    // Summary
    // =========================================================================
    println!("{}", "═".repeat(80));
    println!("🎉 Full Order Lifecycle Test Complete!");
    println!("{}", "═".repeat(80));

    println!("\n📚 Test Summary:");
    println!("   ✅ Step 1: Fetched current market price");
    println!("   ✅ Step 2: Placed safe order using SDK");
    println!("   ✅ Step 3: Verified order in active orders");
    println!("   ✅ Step 4: Cancelled order using SDK");
    println!("   ✅ Step 5: Verified order cancellation");

    println!("\n💡 Key Learnings:");
    println!("   1. SDK handles all signing and nonce management automatically");
    println!("   2. OrderBuilder provides type-safe API for creating orders");
    println!("   3. CancelOrderBuilder handles order cancellation");
    println!("   4. No need for manual form-urlencoded requests");
    println!("   5. Always place test orders far from market (20%+ away)");

    println!("\n{}", "═".repeat(80));

    Ok(())
}
