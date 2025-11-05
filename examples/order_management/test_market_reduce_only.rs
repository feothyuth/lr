//! Test Market Entry + Reduce-Only Exit
//!
//! Enters a small position with a market order, then exits with a reduce-only
//! market order. Useful for validating slippage handling and reduce-only flags
//! against the live API.

#[path = "../common/example_context.rs"]
mod common;

use lighter_client::types::BaseQty;

use common::ExampleContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   MARKET ENTRY + REDUCE-ONLY EXIT TEST                      ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    let ctx = ExampleContext::initialise(Some("test_market_reduce_only")).await?;
    let client = ctx.client();
    let account_index = ctx.account_id().into_inner();
    let api_key_index = ctx.api_key_index().into_inner();
    let market = ctx.market_id();
    let market_index: i32 = market.into();

    println!("📋 Configuration:");
    println!("   Account Index: {}", account_index);
    println!("   API Key Index: {}", api_key_index);
    println!("   Market ID: {}", market_index);
    println!();

    let entry_size = 1; // 0.00001 BTC (below minimum of 0.0002, will likely fail)

    // =========================================================================
    // STEP 1: Market Entry
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 1: Market Entry");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("📝 Order Details:");
    println!("   Type: Market Order");
    println!("   Side: BUY (LONG)");
    println!("   Size: {} units (0.00001 BTC)", entry_size);
    println!();
    println!("⚠️  NOTE: Size is below minimum (0.0002 BTC)");
    println!("   This will likely be rejected");
    println!();

    println!("📡 Submitting market entry...");
    let entry_result = client
        .order(market)
        .buy()
        .qty(BaseQty::try_from(entry_size)?)
        .market()
        .submit()
        .await;

    let entry_tx = match entry_result {
        Ok(submission) => {
            println!("   ✅ Market entry executed!");
            println!("   TX: {}", submission.response().tx_hash);
            Some(submission.response().tx_hash.clone())
        }
        Err(e) => {
            println!("   ❌ Market entry failed: {}", e);
            None
        }
    };

    if entry_tx.is_none() {
        println!();
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("⚠️  Entry failed - trying with minimum size (0.0002 BTC = 20 units)");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!();

        let min_size = 20; // 0.0002 BTC
        println!("📡 Submitting market entry with minimum size...");

        match client
            .order(market)
            .buy()
            .qty(BaseQty::try_from(min_size)?)
            .market()
            .submit()
            .await
        {
            Ok(submission) => {
                println!("   ✅ Market entry executed!");
                println!("   TX: {}", submission.response().tx_hash);
                println!();
                println!("⏰ Waiting 5 seconds for position to settle...");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

                // =========================================================================
                // STEP 2: Market Reduce-Only Exit
                // =========================================================================
                println!();
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!("STEP 2: Market Reduce-Only Exit");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();

                println!("📡 Submitting reduce-only market exit...");
                match client
                    .order(market)
                    .sell()
                    .qty(BaseQty::try_from(min_size)?)
                    .market()
                    .reduce_only()
                    .with_slippage(0.05)
                    .ioc()
                    .submit()
                    .await
                {
                    Ok(exit_submission) => {
                        println!("   ✅ Reduce-only exit executed!");
                        println!("   TX: {}", exit_submission.response().tx_hash);
                    }
                    Err(e) => {
                        println!("   ❌ Reduce-only exit failed: {}", e);
                    }
                }
            }
            Err(e) => {
                println!("   ❌ Market entry with minimum size failed: {}", e);
                println!("   Skip reduce-only exit since no position is open.");
            }
        }
    }

    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("✅ Test complete");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    Ok(())
}
