// Close Open Position via LIMIT Reduce-Only Order (IOC-style)
// Automatically fetches position and closes it with aggressive limit order
// Uses aggressive pricing to cross the spread for immediate fill
// Short expiry (10 min) mimics ImmediateOrCancel behavior
// Polls position API to verify closure
// Supports both LONG and SHORT positions

#[path = "../common/example_context.rs"]
mod common;

use anyhow::{Context, Result};
use lighter_client::types::{BaseQty, Expiry, MarketId, Price};
use time::Duration;

use common::ExampleContext;

const MARKET_ID: i32 = 0; // ETH-PERP

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("🎯 Auto Close Position (LIMIT IOC-style Reduce-Only)");
    println!("{}\n", "═".repeat(80));

    // Initialize context using common helper
    println!("📝 Initializing client...");
    let ctx = ExampleContext::initialise(Some("close_position_limit_ioc")).await?;
    let client = ctx.client();
    println!("✅ Client initialized\n");

    // ============================================================================
    // STEP 1: Fetch Current Position
    // ============================================================================
    println!("🔍 Fetching current positions...");
    let details = client.account().details().await?;

    let account = details
        .accounts
        .first()
        .context("No account found in response")?;

    // Find ETH-PERP position (market_id = 0)
    let eth_position = account.positions.iter().find(|p| p.market_id == MARKET_ID);

    let (position_size, sign) = match eth_position {
        Some(pos) => {
            let sign = pos.sign;
            let size: f64 = pos
                .position
                .parse()
                .context("Failed to parse position size")?;

            if size == 0.0 {
                println!("❌ No open position found on ETH-PERP");
                println!("   Nothing to close!");
                return Ok(());
            }

            (size, sign)
        }
        None => {
            println!("❌ No position data found for ETH-PERP");
            return Ok(());
        }
    };

    // Debug: print actual sign value to verify
    println!("🔍 DEBUG: Position sign value = {}", sign);

    // Determine direction and the side needed to CLOSE the position.
    // Convention: SELL closes a LONG; BUY closes a SHORT.
    // FIXED: Correct sign interpretation
    let (is_long, close_is_sell) = match sign {
        1 => (true, true),    // LONG  (sign=1)  -> SELL to close
        -1 => (false, false), // SHORT (sign=-1) -> BUY  to close
        _ => {
            println!("❌ ERROR: Unexpected sign value: {}", sign);
            println!("   Expected: 1 (LONG) or -1 (SHORT)");
            return Ok(());
        }
    };

    println!("✅ Found open position:");
    println!("   Market:    ETH-PERP");
    println!(
        "   Direction: {}",
        if is_long { "LONG 🟢" } else { "SHORT 🔴" }
    );
    println!("   Size:      {} ETH", position_size);
    println!("   Sign:      {}", sign);
    println!(
        "   To close:  {} {} ETH",
        if close_is_sell { "SELL" } else { "BUY" },
        position_size
    );

    // Check minimum size for LIMIT orders
    const MIN_LIMIT_SIZE: f64 = 0.005; // 0.005 ETH minimum for LIMIT orders
    if position_size < MIN_LIMIT_SIZE {
        println!("\n⚠️  WARNING: Position too small for LIMIT orders!");
        println!("   Position:    {} ETH", position_size);
        println!("   LIMIT min:   {} ETH", MIN_LIMIT_SIZE);
        println!("\n💡 Options:");
        println!("   1. Use MARKET order instead: cargo run --example close_position");
        println!("   2. Increase position to >= 0.005 ETH first");
        println!("\n❌ LIMIT orders require minimum 0.005 ETH");
        println!("   MARKET orders work with as little as $0.01 notional (~0.0001 ETH)");
        return Ok(());
    }

    // ============================================================================
    // STEP 2: Fetch Current Market Price
    // ============================================================================
    println!("\n📈 Fetching current market price...");
    let order_book = client.orders().book(MarketId::new(MARKET_ID), 5).await?;

    let best_bid_order = order_book.bids.first().context("No bids in order book")?;
    let best_ask_order = order_book.asks.first().context("No asks in order book")?;

    let best_bid: f64 = best_bid_order
        .price
        .parse()
        .context("Failed to parse best bid")?;
    let best_ask: f64 = best_ask_order
        .price
        .parse()
        .context("Failed to parse best ask")?;

    println!("   Best Bid: ${:.2}", best_bid);
    println!("   Best Ask: ${:.2}", best_ask);
    println!("   Spread: ${:.2}", best_ask - best_bid);

    // Use aggressive limit price to cross the spread for immediate fill
    // SELL (close LONG): Use best_bid (taker, crosses spread)
    // BUY (close SHORT): Use best_ask (taker, crosses spread)
    let limit_price = if close_is_sell {
        best_bid // SELL at best bid (take liquidity)
    } else {
        best_ask // BUY at best ask (take liquidity)
    };

    println!(
        "   Limit Price: ${:.2} (aggressive, crosses spread)",
        limit_price
    );

    // ============================================================================
    // STEP 3: Submit LIMIT Reduce-Only Order (IOC-style)
    // ============================================================================

    println!("\n📤 Submitting LIMIT reduce-only order (IOC-style)...");
    println!("   Type: LIMIT (GoodTillTime)");
    println!("   Size: {} ETH", position_size);
    println!("   Side: {}", if close_is_sell { "SELL" } else { "BUY" });
    println!("   Price: ${:.2}", limit_price);
    println!("   Expiry: 10 minutes (minimum, fills immediately if liquidity available)");
    println!("   Reduce-Only: YES (can only close position)");
    println!();

    // Convert to integer format (ETH has 4 decimals for size, 2 for price)
    let size_int = (position_size.abs() * 10000.0).round() as i64;
    let price_int = (limit_price * 100.0).round() as i64; // ETH: 2 decimal places

    let qty =
        BaseQty::try_from(size_int).map_err(|e| anyhow::anyhow!("Invalid quantity: {}", e))?;

    let submission = if close_is_sell {
        // SELL to close LONG position
        client
            .order(MarketId::new(MARKET_ID))
            .sell()
            .qty(qty)
            .limit(Price::ticks(price_int))
            .expires_at(Expiry::from_now(Duration::minutes(10))) // 10 minute expiry (minimum)
            .reduce_only()
            .submit()
            .await?
    } else {
        // BUY to close SHORT position
        client
            .order(MarketId::new(MARKET_ID))
            .buy()
            .qty(qty)
            .limit(Price::ticks(price_int))
            .expires_at(Expiry::from_now(Duration::minutes(10))) // 10 minute expiry (minimum)
            .reduce_only()
            .submit()
            .await?
    };

    println!("✅ Order submitted!");
    println!("   TX Hash: {}", submission.response().tx_hash);

    // Debug: Print full response
    println!("\n🔍 Debug - Transaction Response:");
    println!("{:#?}", submission.response());

    // ============================================================================
    // STEP 4: Wait and Verify Position Closed
    // ============================================================================
    println!("\n⏳ Waiting for exchange to process fill...");

    // Poll position API up to 10 times with 500ms delays (5 seconds total)
    let mut position_closed = false;
    let mut final_size = position_size;

    for attempt in 1..=10 {
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        println!("🔍 Checking position (attempt {}/10)...", attempt);

        let details_after = client.account().details().await?;
        let account_after = details_after
            .accounts
            .first()
            .context("No account found in response")?;
        let eth_position_after = account_after
            .positions
            .iter()
            .find(|p| p.market_id == MARKET_ID);

        match eth_position_after {
            Some(pos) => {
                let size_after: f64 = pos.position.parse().unwrap_or(0.0);
                final_size = size_after;

                if size_after == 0.0 {
                    position_closed = true;
                    println!("   ✅ Position closed!");
                    break;
                } else if size_after < position_size {
                    println!(
                        "   ⚠️  Partial fill: {} ETH → {} ETH",
                        position_size, size_after
                    );
                } else {
                    println!("   ⏳ Position unchanged: {} ETH", size_after);
                }
            }
            None => {
                position_closed = true;
                final_size = 0.0;
                println!("   ✅ Position closed!");
                break;
            }
        }
    }

    // Show final result
    println!();
    if position_closed {
        println!("{}", "═".repeat(80));
        println!("✅ SUCCESS: Position fully closed!");
        println!("{}", "═".repeat(80));
        println!("   Previous: {} ETH", position_size);
        println!("   Now:      0.0000 ETH");
        println!();
    } else if final_size < position_size {
        println!("{}", "═".repeat(80));
        println!("⚠️  PARTIAL: Position partially closed");
        println!("{}", "═".repeat(80));
        println!("   Previous: {} ETH", position_size);
        println!("   Now:      {} ETH", final_size);
        println!("   Closed:   {} ETH", position_size - final_size);
        println!();
        println!("💡 Reason: Partial liquidity available");
        println!("   Run again to close remaining position");
    } else {
        println!("{}", "═".repeat(80));
        println!("❌ FAILED: Position not closed");
        println!("{}", "═".repeat(80));
        println!("   Position: {} ETH (unchanged)", final_size);
        println!();
        println!("💡 Possible reasons:");
        println!("   1. No matching liquidity at limit price");
        println!("   2. Order resting on book (check active orders)");
        println!("   3. Try MARKET order instead for guaranteed execution");
    }

    println!("\n{}", "═".repeat(80));
    println!("📝 Notes:");
    println!("   - LIMIT at best bid/ask = aggressive taker order");
    println!("   - Reduce-only ensures order can only close existing position");
    println!("   - 10 minute expiry (minimum allowed by exchange)");
    println!("   - If doesn't fill: order may be resting on book");
    println!("   - Check active orders: cargo run --example monitor_active_orders");
    println!("   - For guaranteed execution: use MARKET order with slippage");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
