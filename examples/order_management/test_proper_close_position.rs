//! Proper Position Close Test
//!
//! 1. Enter position with market order
//! 2. Verify position created
//! 3. Close with market reduce-only (WITH SLIPPAGE!)
//! 4. Verify position closed
//!
//! Run with:
//! LIGHTER_PRIVATE_KEY="..." ACCOUNT_INDEX="..." LIGHTER_API_KEY_INDEX="0" \
//! cargo run --example test_proper_close_position

use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, BaseQty, MarketId},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   PROPER POSITION CLOSE TEST - WITH SLIPPAGE                ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    // Load configuration
    let private_key = std::env::var("LIGHTER_PRIVATE_KEY").expect("LIGHTER_PRIVATE_KEY not set");
    let account_index: i64 = std::env::var("ACCOUNT_INDEX")
        .expect("ACCOUNT_INDEX not set")
        .parse()?;
    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")
        .unwrap_or_else(|_| "0".to_string())
        .parse()?;

    println!("📋 Configuration:");
    println!("   Account Index: {}", account_index);
    println!("   API Key Index: {}", api_key_index);
    println!("   Market: BTC-PERP (ID: 1)");
    println!();

    // Build client
    let api_url = "https://mainnet.zklighter.elliot.ai";
    let client = LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    println!("✅ Client initialized");
    println!();

    let market = MarketId::new(1); // BTC-PERP
    let market_id = 1;
    let size = 20; // 0.0002 BTC (minimum)

    // =========================================================================
    // STEP 1: Check existing position
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 1: Check Existing Position");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    let details_before = client.account().details().await?;
    let account = details_before.accounts.first().ok_or("No account found")?;

    let existing_position = account.positions.iter().find(|p| p.market_id == market_id);

    match existing_position {
        Some(pos) => {
            let pos_size: f64 = pos.position.parse().unwrap_or(0.0);
            if pos_size != 0.0 {
                println!("⚠️  Existing position found:");
                println!("   Size: {} BTC", pos_size);
                println!("   Sign: {}", pos.sign);
                println!("   Skipping entry, will close existing position");
                println!();

                // Skip to close
                let close_is_sell = pos.sign == 1; // LONG = sign 1, close with SELL
                let pos_size_int = (pos_size.abs() * 100_000.0).round() as i64;

                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!("CLOSING EXISTING POSITION");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();
                println!(
                    "   Direction: {}",
                    if close_is_sell {
                        "SELL (close LONG)"
                    } else {
                        "BUY (close SHORT)"
                    }
                );
                println!("   Size: {} units", pos_size_int);
                println!("   Slippage: 5%");
                println!();

                let close_result = if close_is_sell {
                    client
                        .order(market)
                        .sell()
                        .qty(BaseQty::try_from(pos_size_int)?)
                        .market()
                        .with_slippage(0.05) // CRITICAL!
                        .reduce_only()
                        .submit()
                        .await
                } else {
                    client
                        .order(market)
                        .buy()
                        .qty(BaseQty::try_from(pos_size_int)?)
                        .market()
                        .with_slippage(0.05) // CRITICAL!
                        .reduce_only()
                        .submit()
                        .await
                };

                match close_result {
                    Ok(submission) => {
                        println!(
                            "✅ Close order submitted: {}",
                            submission.response().tx_hash
                        );
                        verify_position_closed(&client, market_id).await;
                    }
                    Err(e) => {
                        println!("❌ Close failed: {}", e);
                    }
                }

                return Ok(());
            } else {
                println!("✅ No existing position (size = 0)");
            }
        }
        None => {
            println!("✅ No existing position");
        }
    }
    println!();

    // =========================================================================
    // STEP 2: Market Entry
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 2: Market Entry");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("📝 Order Details:");
    println!("   Type: Market Order");
    println!("   Side: BUY (LONG)");
    println!("   Size: {} units (0.0002 BTC)", size);
    println!("   Slippage: 5%");
    println!();

    println!("📡 Submitting market entry...");
    let entry = client
        .order(market)
        .buy()
        .qty(BaseQty::try_from(size)?)
        .market()
        .with_slippage(0.05) // CRITICAL!
        .submit()
        .await?;

    println!("✅ Entry executed: {}", entry.response().tx_hash);
    println!();

    // =========================================================================
    // STEP 3: Verify Position Created
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 3: Verify Position Created");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("⏰ Waiting 3 seconds for position to settle...");
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    let mut position_created = false;
    let mut position_size: f64 = 0.0;

    for attempt in 1..=5 {
        println!("🔍 Checking position (attempt {}/5)...", attempt);

        let details_after = client.account().details().await?;
        let account_after = details_after.accounts.first().ok_or("No account found")?;

        let btc_position = account_after
            .positions
            .iter()
            .find(|p| p.market_id == market_id);

        match btc_position {
            Some(pos) => {
                let pos_size: f64 = pos.position.parse().unwrap_or(0.0);
                if pos_size > 0.0 {
                    position_created = true;
                    position_size = pos_size;
                    println!("   ✅ Position created!");
                    println!("   Size: {} BTC", pos_size);
                    println!("   Sign: {} (LONG)", pos.sign);
                    break;
                }
            }
            None => {}
        }

        if attempt < 5 {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }

    if !position_created {
        println!("   ❌ Position not detected after 5 attempts");
        println!("   Test failed - cannot continue to close");
        return Ok(());
    }

    println!();

    // =========================================================================
    // STEP 4: Market Reduce-Only Close (WITH SLIPPAGE!)
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 4: Market Reduce-Only Close (WITH SLIPPAGE)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("📝 Close Order Details:");
    println!("   Type: Market Order (Reduce-Only)");
    println!("   Side: SELL (close LONG position)");
    println!("   Size: {} units", size);
    println!("   Slippage: 5% ⚠️  CRITICAL FOR MARKET ORDERS!");
    println!("   Reduce-Only: YES");
    println!();

    println!("📡 Submitting market reduce-only close...");
    let close = client
        .order(market)
        .sell() // Opposite side to close LONG
        .qty(BaseQty::try_from(size)?)
        .market()
        .with_slippage(0.05) // ⚠️  THIS IS CRITICAL!
        .reduce_only()
        .submit()
        .await?;

    println!("✅ Close order submitted: {}", close.response().tx_hash);
    println!();

    // =========================================================================
    // STEP 5: Verify Position Closed
    // =========================================================================
    verify_position_closed(&client, market_id).await;

    Ok(())
}

async fn verify_position_closed(
    client: &lighter_client::lighter_client::LighterClient,
    market_id: i32,
) {
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("STEP 5: Verify Position Closed");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("⏰ Waiting for fill to settle...");

    let mut position_closed = false;

    for attempt in 1..=10 {
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        println!("🔍 Checking position (attempt {}/10)...", attempt);

        match client.account().details().await {
            Ok(details) => {
                if let Some(account) = details.accounts.first() {
                    let btc_position = account.positions.iter().find(|p| p.market_id == market_id);

                    match btc_position {
                        Some(pos) => {
                            let pos_size: f64 = pos.position.parse().unwrap_or(0.0);
                            if pos_size == 0.0 {
                                position_closed = true;
                                println!("   ✅ Position closed!");
                                break;
                            } else {
                                println!("   ⏳ Position still open: {} BTC", pos_size);
                            }
                        }
                        None => {
                            position_closed = true;
                            println!("   ✅ Position closed (not in list)!");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                println!("   ⚠️  API error: {}", e);
            }
        }
    }

    println!();
    if position_closed {
        println!("═══════════════════════════════════════════════════════════════");
        println!("✅ TEST PASSED: Position successfully closed!");
        println!("═══════════════════════════════════════════════════════════════");
        println!();
        println!("🔑 Key learning:");
        println!("   Market orders REQUIRE .with_slippage(0.05) to fill!");
        println!("   Without slippage tolerance, orders are rejected/cancelled");
    } else {
        println!("═══════════════════════════════════════════════════════════════");
        println!("❌ TEST FAILED: Position not closed after 5 seconds");
        println!("═══════════════════════════════════════════════════════════════");
        println!();
        println!("💡 Possible reasons:");
        println!("   1. Insufficient liquidity");
        println!("   2. Position too small");
        println!("   3. Market order rejected");
    }
}
