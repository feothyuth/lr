// Close Position via WebSocket - LIMIT IOC Order with Reduce-Only
// Uses SDK's SignerClient + WebSocket for order submission
// Fetches position via REST, signs with SDK signer, sends via WebSocket
// LIMIT order with aggressive pricing to cross spread for immediate fill
// FIXED: Correct sign interpretation (sign=1 is LONG, sign=-1 is SHORT)

#[path = "../common/example_context.rs"]
mod common;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use lighter_client::types::MarketId;
use tokio::time::{timeout, Duration, Instant};

use common::{submit_signed_payload, ExampleContext};

const MARKET_ID: i32 = 0; // ETH-PERP

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("🎯 Close Position via WebSocket (LIMIT IOC-style Reduce-Only)");
    println!("    SDK SignerClient + WebSocket Order Submission");
    println!("{}\n", "═".repeat(80));

    // Initialize context using common helper
    println!("📝 Initializing client...");
    let ctx = ExampleContext::initialise(Some("close_position_limit_ioc_ws")).await?;
    let client = ctx.client();
    let signer = ctx.signer()?;
    println!("✅ Client initialized\n");

    // ============================================================================
    // STEP 1: Fetch Position (REST API for data only)
    // ============================================================================
    println!("🔍 Fetching current position via REST...");
    let details = client.account().details().await?;
    let account = details.accounts.first().context("No account found")?;

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
                return Ok(());
            }

            (size, sign)
        }
        None => {
            println!("❌ No position data found for ETH-PERP");
            return Ok(());
        }
    };

    println!("🔍 DEBUG: Position sign value = {}", sign);

    // FIXED: Correct sign interpretation
    let (is_long, is_ask) = match sign {
        1 => (true, true),    // LONG  (sign=1)  -> SELL (is_ask=true) to close
        -1 => (false, false), // SHORT (sign=-1) -> BUY  (is_ask=false) to close
        _ => {
            println!("❌ ERROR: Unexpected sign value: {}", sign);
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
        "   To close:  {} {} ETH\n",
        if is_ask { "SELL" } else { "BUY" },
        position_size
    );

    // Check minimum size for LIMIT orders
    const MIN_LIMIT_SIZE: f64 = 0.005;
    if position_size < MIN_LIMIT_SIZE {
        println!("⚠️  WARNING: Position too small for LIMIT orders!");
        println!("   Position:    {} ETH", position_size);
        println!("   LIMIT min:   {} ETH", MIN_LIMIT_SIZE);
        println!("\n💡 Use MARKET order instead: cargo run --example close_position_ws");
        println!("❌ LIMIT orders require minimum 0.005 ETH\n");
        return Ok(());
    }

    // ============================================================================
    // STEP 2: Fetch Market Prices (REST for pricing data)
    // ============================================================================
    println!("📈 Fetching market prices via REST...");
    let order_book = client.orders().book(MarketId::new(MARKET_ID), 5).await?;

    let best_bid: f64 = order_book.bids.first().context("No bids")?.price.parse()?;
    let best_ask: f64 = order_book.asks.first().context("No asks")?.price.parse()?;

    println!("   Best Bid: ${:.2}", best_bid);
    println!("   Best Ask: ${:.2}", best_ask);
    println!("   Spread: ${:.2}", best_ask - best_bid);

    // Use aggressive limit price to cross spread (taker order)
    let limit_price = if is_ask {
        best_bid // SELL at best bid (take liquidity)
    } else {
        best_ask // BUY at best ask (take liquidity)
    };

    println!(
        "   Limit Price: ${:.2} (aggressive, crosses spread)\n",
        limit_price
    );

    // ============================================================================
    // STEP 3: Initialize SDK Signer and Sign Order
    // ============================================================================
    println!("{}", "═".repeat(80));
    println!("📡 Using SDK SignerClient + WebSocket");
    println!("{}", "═".repeat(80));
    println!();

    println!("✍️  Signing LIMIT reduce-only order...");

    let size_int = (position_size * 10000.0) as i64; // ETH: 4 decimals
    let price_int = (limit_price * 100.0) as i32; // ETH: 2 decimals (note: i32 for SDK)

    let client_order_index = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;

    // Calculate order expiry (10 minutes from now)
    let order_expiry = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64
        + 600_000; // 10 minutes = 600,000ms

    let signed_payload = signer
        .sign_create_order(
            MARKET_ID,
            client_order_index,
            size_int,
            price_int,
            is_ask,
            signer.order_type_limit(),
            signer.order_time_in_force_good_till_time(),
            true, // reduce_only
            0,    // trigger_price
            order_expiry,
            None, // nonce (SDK manages it)
            None, // api_key_index (SDK manages it)
        )
        .await?;

    println!("✅ Order signed via SDK\n");

    println!("📊 Order details:");
    println!("   Type: LIMIT (GoodTillTime)");
    println!("   Side: {}", if is_ask { "SELL" } else { "BUY" });
    println!("   Size: {} ETH", position_size);
    println!("   Limit price: ${:.2} (crosses spread)", limit_price);
    println!("   Expiry: 10 minutes");
    println!("   Reduce-Only: YES\n");

    // ============================================================================
    // STEP 4: Send via WebSocket
    // ============================================================================

    // Connect to WebSocket using SDK
    println!("🌐 Connecting to WebSocket...");
    let ws_start = Instant::now();

    // Build WebSocket with authentication
    let ws_builder = ctx.ws_builder();
    let (mut stream, _auth_token) = common::connect_private_stream(&ctx, ws_builder).await?;

    println!(
        "✅ Connected and authenticated in {:?}\n",
        ws_start.elapsed()
    );

    // Send transaction via WebSocket
    println!("📤 Sending order via WebSocket...");
    let send_start = Instant::now();

    let success = submit_signed_payload(stream.connection_mut(), &signed_payload).await?;
    let send_duration = send_start.elapsed();

    if success {
        println!("✅ Order sent in {:?}\n", send_duration);
    } else {
        println!("❌ Order submission failed in {:?}\n", send_duration);
    }

    // Wait for response
    println!("⏳ Waiting for exchange response...\n");

    let response_start = Instant::now();
    let deadline = Duration::from_millis(5000);
    let mut found_response = false;

    while response_start.elapsed() < deadline {
        match timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(event)) => {
                use lighter_client::ws_client::WsEvent;

                match event {
                    Ok(WsEvent::Transaction(tx)) => {
                        let response_time = response_start.elapsed();
                        println!("✅ Order ACCEPTED in {:?}", response_time);
                        println!("📩 Transaction: {:#?}", tx);
                        found_response = true;
                        break;
                    }
                    Ok(WsEvent::ExecutedTransaction(exec)) => {
                        let response_time = response_start.elapsed();
                        println!("✅ Order EXECUTED in {:?}", response_time);
                        println!("📩 Executed transaction: {:#?}", exec);
                        found_response = true;
                        break;
                    }
                    Ok(WsEvent::Pong) => {
                        // Ignore pong messages
                        continue;
                    }
                    Ok(WsEvent::Unknown(text)) => {
                        // Check if it's an error or acceptance message
                        if text.contains("\"code\":200") {
                            let response_time = response_start.elapsed();
                            println!("✅ Order ACCEPTED in {:?}", response_time);
                            println!("📩 Response: {}", text);
                            found_response = true;
                            break;
                        } else if text.contains("\"error\":") || text.contains("\"type\":\"error\"")
                        {
                            println!("❌ Order REJECTED: {}", text);
                            found_response = true;
                            break;
                        }
                    }
                    Ok(_) => {
                        // Ignore other events
                        continue;
                    }
                    Err(e) => {
                        println!("❌ WebSocket error: {}", e);
                        found_response = true;
                        break;
                    }
                }
            }
            Ok(None) => {
                println!("❌ WebSocket closed");
                break;
            }
            Err(_) => {
                // Timeout, continue waiting
                continue;
            }
        }
    }

    if !found_response {
        println!("⏳ Timeout: No response within 5 seconds");
    }

    println!();

    // Verify position closed (via REST)
    println!("🔍 Verifying position closed (via REST)...");
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    let details_after = client.account().details().await?;
    let account_after = details_after.accounts.first().context("No account found")?;
    let position_after = account_after
        .positions
        .iter()
        .find(|p| p.market_id == MARKET_ID);

    let position_closed = match position_after {
        Some(pos) => {
            let size_after: f64 = pos.position.parse().unwrap_or(0.0);
            size_after == 0.0
        }
        None => true,
    };

    println!();
    if position_closed {
        println!("{}", "═".repeat(80));
        println!("✅ SUCCESS: Position closed via WebSocket!");
        println!("{}", "═".repeat(80));
        println!("   Previous: {} ETH", position_size);
        println!("   Now:      0.0000 ETH");
    } else {
        println!("{}", "═".repeat(80));
        println!("⚠️  Position still open");
        println!("{}", "═".repeat(80));
        println!("💡 Possible reasons:");
        println!("   1. Order resting on book (no matching liquidity)");
        println!("   2. Wait longer for fill");
        println!("   3. Try MARKET order for guaranteed execution");
    }

    println!("\n📊 Performance:");
    println!("   Order submission: {:?}", send_duration);
    println!(
        "   Total (WS connect + auth + send): {:?}\n",
        ws_start.elapsed()
    );

    println!("{}", "═".repeat(80));
    println!("✅ WebSocket LIMIT IOC test complete!");
    println!("   SDK SignerClient for signing");
    println!("   SDK WebSocket for order submission");
    println!("   LIMIT order with aggressive pricing (crosses spread)");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
