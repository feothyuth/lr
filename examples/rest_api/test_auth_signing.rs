// Example: Authentication and transaction signing with lighter_client SDK
//
// This example demonstrates how the SDK handles authentication and signing internally.
// You no longer need to:
// 1. Manually manage Go FFI library calls
// 2. Sign CreateOrder/CancelOrder transactions manually
// 3. Create authentication tokens manually
//
// The SDK handles all of this automatically through its high-level API.
//
// This example shows:
// 1. How to sign orders without submitting (.sign() instead of .submit())
// 2. How cancel operations work
// 3. How the SDK simplifies the entire flow

use anyhow::Result;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, BaseQty, Expiry, MarketId, Price},
};
#[path = "../common/env.rs"]
mod env;

use env::resolve_api_url;
use time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("🔐 SDK Authentication and Signing Examples");
    println!("{}\n", "═".repeat(80));

    // Load environment variables
    dotenvy::dotenv().ok();

    let http_url = resolve_api_url("https://mainnet.zklighter.elliot.ai");
    let private_key =
        std::env::var("LIGHTER_PRIVATE_KEY").expect("LIGHTER_PRIVATE_KEY must be set in .env");
    let account_index: i64 = std::env::var("LIGHTER_ACCOUNT_INDEX")
        .or_else(|_| std::env::var("ACCOUNT_INDEX"))
        .unwrap_or_else(|_| "0".to_string())
        .parse()?;
    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")
        .or_else(|_| std::env::var("API_KEY_INDEX"))
        .unwrap_or_else(|_| "0".to_string())
        .parse()?;

    println!("🔐 Initializing SDK client...");
    println!("   HTTP URL: {}", http_url);
    println!("   Account Index: {}", account_index);
    println!("   API Key Index: {}", api_key_index);

    let client = LighterClient::builder()
        .api_url(http_url)
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    println!("✅ SDK client initialized successfully!\n");

    // ========== Example 1: Sign Order Without Submitting ==========
    println!("{}", "─".repeat(80));
    println!("📝 Example 1: Sign Buy Limit Order (without submitting)");
    println!("{}", "─".repeat(80));

    let market = MarketId::new(0); // ETH-PERP
    let size = 5000; // 0.5 ETH with 4 decimals
    let price = 300000; // $3000 with 2 decimals

    println!("   Order Params:");
    println!("   - Market: 0 (ETH)");
    println!("   - Side: BUY");
    println!("   - Size (scaled): {}", size);
    println!("   - Price (scaled): {}", price);
    println!("   - Type: Limit Post-Only");

    // Sign order without submitting
    let signed = client
        .order(market)
        .buy()
        .qty(BaseQty::try_from(size).map_err(|e| anyhow::anyhow!("{}", e))?)
        .limit(Price::ticks(price))
        .expires_at(Expiry::from_now(Duration::minutes(15)))
        .post_only()
        .sign()
        .await?;

    println!("   ✅ Order signed successfully!");
    println!(
        "   Signed payload (first 100 chars): {}...",
        &signed.payload()[..100.min(signed.payload().len())]
    );
    println!("   Full payload length: {} bytes\n", signed.payload().len());

    // ========== Example 2: Sign Sell Limit Order ==========
    println!("{}", "─".repeat(80));
    println!("📝 Example 2: Sign Sell Limit Order (without submitting)");
    println!("{}", "─".repeat(80));

    let signed = client
        .order(market)
        .sell()
        .qty(BaseQty::try_from(size).map_err(|e| anyhow::anyhow!("{}", e))?)
        .limit(Price::ticks(310000))
        .expires_at(Expiry::from_now(Duration::minutes(15)))
        .post_only()
        .sign()
        .await?;

    println!("   Order Params:");
    println!("   - Market: 0 (ETH)");
    println!("   - Side: SELL");
    println!("   - Size (scaled): {}", size);
    println!("   - Price (scaled): 310000");
    println!("   - Type: Limit Post-Only");
    println!("   ✅ Order signed successfully!");
    println!("   Payload length: {} bytes\n", signed.payload().len());

    // ========== Example 3: Market Order (Close Position) ==========
    println!("{}", "─".repeat(80));
    println!("📝 Example 3: Sign Market Order (Close Long Position)");
    println!("{}", "─".repeat(80));

    let signed = client
        .order(market)
        .sell()
        .qty(BaseQty::try_from(size).map_err(|e| anyhow::anyhow!("{}", e))?)
        .market()
        .reduce_only()
        .ioc()
        .sign()
        .await?;

    println!("   Order Params:");
    println!("   - Market: 0 (ETH)");
    println!("   - Side: SELL");
    println!("   - Size (scaled): {}", size);
    println!("   - Type: Market");
    println!("   - Time in Force: IOC");
    println!("   - Reduce Only: true");
    println!("   ✅ Market order signed successfully!");
    println!("   Payload length: {} bytes\n", signed.payload().len());

    // ========== Example 4: Cancel Order ==========
    println!("{}", "─".repeat(80));
    println!("📝 Example 4: Cancel Order");
    println!("{}", "─".repeat(80));

    let order_id_to_cancel = 12345; // Example order ID
    let market_to_cancel = MarketId::new(0);

    println!("   Cancel Params:");
    println!("   - Market: 0 (ETH)");
    println!("   - Order ID: {}", order_id_to_cancel);

    println!("\n   Note: CancelOrderBuilder only supports .submit(), not .sign()");
    println!("   This is because cancel operations are typically executed immediately.");
    println!("   Use .submit() to cancel an order:\n");
    println!("   Example:");
    println!("   let submission = client.cancel(market, order_id).submit().await?;");
    println!();

    // ========== Example 5: Cancel All Orders ==========
    println!("{}", "─".repeat(80));
    println!("📝 Example 5: Cancel All Orders");
    println!("{}", "─".repeat(80));

    println!("   Note: CancelAllBuilder only supports .submit(), not .sign()");
    println!("   This is because cancel_all operations are typically executed immediately.");
    println!("   Use .submit() to cancel all orders:\n");
    println!("   Example:");
    println!("   let submission = client.cancel_all().submit().await?;");
    println!();

    // ========== Example 6: Submit vs Sign ==========
    println!("{}", "─".repeat(80));
    println!("📝 Example 6: Understanding .sign() vs .submit()");
    println!("{}", "─".repeat(80));

    println!("   The SDK provides two methods for orders:");
    println!();
    println!("   1. .sign() - Signs the transaction and returns the signed payload");
    println!("      - Use when you want to inspect the signed data");
    println!("      - Use when you want to submit manually later");
    println!("      - Returns: SignedPayload");
    println!();
    println!("   2. .submit() - Signs AND submits the transaction to the API");
    println!("      - Use for normal order placement");
    println!("      - Handles all API communication automatically");
    println!("      - Returns: OrderSubmission with response details");
    println!();

    // ========== Example 7: Different Order Types ==========
    println!("{}", "─".repeat(80));
    println!("📝 Example 7: Different Order Types and Time in Force");
    println!("{}", "─".repeat(80));

    println!("   Available order configurations:");
    println!();
    println!("   Limit Orders:");
    println!("   - .limit(price).post_only()  // Post-only limit");
    println!("   - .limit(price).ioc()        // Immediate or Cancel");
    println!("   - .limit(price)              // Good till expiry");
    println!();
    println!("   Market Orders:");
    println!("   - .market()                  // Market order");
    println!("   - .market().ioc()            // Market IOC");
    println!();
    println!("   Position Management:");
    println!("   - .reduce_only()             // Only reduce position");
    println!();
    println!("   Expiry:");
    println!("   - .expires_at(Expiry::from_now(Duration::minutes(5)))");
    println!("   - .expires_at(Expiry::from_now(Duration::days(7)))");
    println!();

    // ========== Summary ==========
    println!("{}", "═".repeat(80));
    println!("🎉 All signing examples completed!");
    println!("{}", "═".repeat(80));

    println!("\n💡 Key Takeaways:");
    println!("   1. SDK handles all signing internally - no manual FFI calls needed");
    println!("   2. Use .sign() to get signed payload without submitting");
    println!("   3. Use .submit() to sign and send in one step");
    println!("   4. SDK manages nonces automatically");
    println!("   5. SDK manages authentication tokens automatically");
    println!("   6. Type-safe API prevents common errors");
    println!();

    println!("Next steps:");
    println!("1. Use .submit() for actual order placement");
    println!("2. Use .sign() when you need to inspect signed data");
    println!("3. Check the other examples for full order lifecycle");
    println!();

    Ok(())
}
