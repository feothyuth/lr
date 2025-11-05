//! Cancel-Replace Pattern for Market Making
//!
//! Core MM operation: Cancel old quotes and place new quotes
//! This is THE fundamental operation for updating quotes in market making

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::{connect_private_stream, sign_cancel_all_for_ws, ExampleContext};
use lighter_client::types::{ApiKeyIndex, BaseQty, MarketId, Nonce, Price};
use serde_json::Value;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::timeout;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("🔄 Cancel-Replace Pattern (Market Making)");
    println!("{}\n", "═".repeat(80));

    // Initialize context
    let ctx = ExampleContext::initialise(Some("test_cancel_replace")).await?;
    let market = ctx.market_id();
    let client = ctx.client();

    println!("✅ Initialized\n");

    // Fetch current market price
    println!("📈 Fetching current market price...");
    let order_book = client.orders().book(market, 25).await?;
    let (best_bid, best_ask) = order_book
        .bids
        .first()
        .map(|bid| bid.price.parse::<f64>().unwrap_or_default())
        .zip(
            order_book
                .asks
                .first()
                .map(|ask| ask.price.parse::<f64>().unwrap_or_default()),
        )
        .unwrap_or((3900.0, 3900.0));

    let mid = (best_bid + best_ask) / 2.0;
    println!(
        "✅ Market: bid=${:.2}, ask=${:.2}, mid=${:.2}\n",
        best_bid, best_ask, mid
    );

    // Connect to WebSocket
    println!("🌐 Connecting to WebSocket...");
    let builder = client.ws().subscribe_transactions();
    let (mut stream, _) = connect_private_stream(&ctx, builder).await?;
    println!("✅ Connected and authenticated\n");

    println!("{}", "═".repeat(80));
    println!("CANCEL-REPLACE CYCLE");
    println!("{}\n", "═".repeat(80));

    let cycle_start = Instant::now();

    // ========== STEP 1: Place Initial Orders ==========
    println!("📍 STEP 1: Place initial quotes (will be replaced)");
    println!("{}", "─".repeat(80));

    let initial_bid = mid * 0.98; // 2% below mid
    let initial_ask = mid * 1.02; // 2% above mid
    let size = 0.005; // Minimum size

    println!(
        "Initial BID: ${:.2} | ASK: ${:.2}\n",
        initial_bid, initial_ask
    );

    let step1_start = Instant::now();

    // Sign both orders
    let (bid_signed, ask_signed) =
        sign_order_pair(&ctx, market.into(), size, initial_bid, initial_ask).await?;

    // Send both orders in parallel
    let parallel_send = Instant::now();
    let connection = stream.connection_mut();

    send_signed_order(connection, &bid_signed).await?;
    send_signed_order(connection, &ask_signed).await?;

    println!(
        "  Sent both orders in {}ms",
        parallel_send.elapsed().as_millis()
    );

    // Wait for both responses
    let wait_start = Instant::now();
    let bid_success = timeout(
        Duration::from_secs(3),
        connection.wait_for_tx_response(Duration::from_secs(3)),
    )
    .await
    .unwrap_or(Ok(false))
    .unwrap_or(false);

    println!(
        "  BID: {} ({}ms)",
        if bid_success {
            "✅ Placed"
        } else {
            "❌ Failed"
        },
        wait_start.elapsed().as_millis()
    );

    let ask_success = timeout(
        Duration::from_secs(3),
        connection.wait_for_tx_response(Duration::from_secs(3)),
    )
    .await
    .unwrap_or(Ok(false))
    .unwrap_or(false);

    println!(
        "  ASK: {} ({}ms from start)",
        if ask_success {
            "✅ Placed"
        } else {
            "❌ Failed"
        },
        wait_start.elapsed().as_millis()
    );

    let step1_duration = step1_start.elapsed();
    println!("  Step 1 Total: {}ms\n", step1_duration.as_millis());

    // Small delay to ensure orders are on the book before canceling
    println!("⏳ Brief delay (100ms) to ensure orders are posted...\n");
    tokio::time::sleep(Duration::from_millis(100)).await;

    // ========== STEP 2: Cancel All Orders ==========
    println!("📍 STEP 2: Cancel old quotes");
    println!("{}", "─".repeat(80));

    let step2_start = Instant::now();

    // Sign and send cancel all
    let (cancel_tx_type, cancel_tx_info) = sign_cancel_all_for_ws(&ctx, 0, 0).await?;
    let cancel_send = Instant::now();

    let cancel_payload: Value = serde_json::from_str(&cancel_tx_info)?;
    connection
        .send_transaction(cancel_tx_type, cancel_payload)
        .await?;

    let cancel_success = timeout(
        Duration::from_secs(3),
        connection.wait_for_tx_response(Duration::from_secs(3)),
    )
    .await
    .unwrap_or(Ok(false))
    .unwrap_or(false);

    let cancel_duration = cancel_send.elapsed();
    println!(
        "  Cancel ALL: {} ({}ms)",
        if cancel_success {
            "✅ Cancelled"
        } else {
            "❌ Failed"
        },
        cancel_duration.as_millis()
    );

    let step2_duration = step2_start.elapsed();
    println!("  Step 2 Total: {}ms\n", step2_duration.as_millis());

    // ========== STEP 3: Place New Orders ==========
    println!("📍 STEP 3: Place new quotes (tighter spread)");
    println!("{}", "─".repeat(80));

    let new_bid = mid * 0.99; // 1% below mid (tighter)
    let new_ask = mid * 1.01; // 1% above mid (tighter)

    println!("New BID: ${:.2} | ASK: ${:.2}\n", new_bid, new_ask);

    let step3_start = Instant::now();

    // Sign both new orders
    let (bid2_signed, ask2_signed) =
        sign_order_pair(&ctx, market.into(), size, new_bid, new_ask).await?;

    // Send both new orders in parallel
    let parallel_send2 = Instant::now();
    send_signed_order(connection, &bid2_signed).await?;
    send_signed_order(connection, &ask2_signed).await?;

    println!(
        "  Sent both orders in {}ms",
        parallel_send2.elapsed().as_millis()
    );

    // Wait for both responses
    let wait_start2 = Instant::now();
    let bid2_success = timeout(
        Duration::from_secs(3),
        connection.wait_for_tx_response(Duration::from_secs(3)),
    )
    .await
    .unwrap_or(Ok(false))
    .unwrap_or(false);

    println!(
        "  BID: {} ({}ms)",
        if bid2_success {
            "✅ Placed"
        } else {
            "❌ Failed"
        },
        wait_start2.elapsed().as_millis()
    );

    let ask2_success = timeout(
        Duration::from_secs(3),
        connection.wait_for_tx_response(Duration::from_secs(3)),
    )
    .await
    .unwrap_or(Ok(false))
    .unwrap_or(false);

    println!(
        "  ASK: {} ({}ms from start)",
        if ask2_success {
            "✅ Placed"
        } else {
            "❌ Failed"
        },
        wait_start2.elapsed().as_millis()
    );

    let step3_duration = step3_start.elapsed();
    println!("  Step 3 Total: {}ms\n", step3_duration.as_millis());

    let cycle_duration = cycle_start.elapsed();

    println!("{}", "═".repeat(80));
    println!("✅ Cancel-Replace cycle complete!");
    println!("{}", "═".repeat(80));
    println!();
    println!("📊 Latency Breakdown:");
    println!(
        "   Step 1 (Place initial):  {}ms",
        step1_duration.as_millis()
    );
    println!(
        "   Step 2 (Cancel all):     {}ms",
        step2_duration.as_millis()
    );
    println!(
        "   Step 3 (Place new):      {}ms",
        step3_duration.as_millis()
    );
    println!("   ─────────────────────────────");
    println!(
        "   Total Cycle Time:        {}ms",
        cycle_duration.as_millis()
    );
    println!();
    println!("💡 Key Points:");
    println!("   1. ⚡ PARALLEL: Send both bid+ask without waiting (2x faster!)");
    println!("   2. Cancel old orders before placing new (avoid duplicates)");
    println!("   3. Wait for cancel confirmation");
    println!("   4. Place new orders with updated prices");
    println!("   5. This is the core loop for market making bots");
    println!();
    println!("⚡ Performance:");
    println!("   Actual cycle: {}ms", cycle_duration.as_millis());
    println!(
        "   Theoretical max rate: {:.1} Hz",
        1000.0 / cycle_duration.as_millis() as f64
    );
    println!();
    println!("📈 Optimization Impact:");
    println!("   Sequential (old): ~630ms per cycle (1.6 Hz)");
    println!(
        "   Parallel (new):   ~{}ms per cycle ({:.1} Hz) ⚡",
        cycle_duration.as_millis(),
        1000.0 / cycle_duration.as_millis() as f64
    );
    println!(
        "   Improvement:      ~{}% faster!",
        ((630.0 - cycle_duration.as_millis() as f64) / 630.0 * 100.0) as i32
    );
    println!();
    println!("🎯 Target for production MM bot: 50-200ms (20-5 Hz)");
    println!("{}\n", "═".repeat(80));

    Ok(())
}

/// Signs a pair of bid and ask limit orders.
/// Returns (bid_signed, ask_signed) tuple.
async fn sign_order_pair(
    ctx: &ExampleContext,
    market_id: i32,
    size: f64,
    bid_price: f64,
    ask_price: f64,
) -> Result<(String, String)> {
    let client = ctx.client();
    let signer = ctx.signer()?;

    let price_bid_ticks = Price::ticks((bid_price * 100.0).round() as i64);
    let price_ask_ticks = Price::ticks((ask_price * 100.0).round() as i64);
    let base_qty = BaseQty::try_from((size * 10_000.0).round() as i64)
        .map_err(|e| anyhow::anyhow!("Invalid size: {}", e))?;

    let (bid_api_key, bid_nonce) = signer.next_nonce().await?;
    let bid_signed = client
        .order(MarketId::new(market_id))
        .buy()
        .qty(base_qty.clone())
        .limit(price_bid_ticks)
        .post_only()
        .with_api_key(ApiKeyIndex::new(bid_api_key))
        .with_nonce(Nonce::new(bid_nonce))
        .sign()
        .await?;

    let (ask_api_key, ask_nonce) = signer.next_nonce().await?;
    let ask_signed = client
        .order(MarketId::new(market_id))
        .sell()
        .qty(base_qty)
        .limit(price_ask_ticks)
        .post_only()
        .with_api_key(ApiKeyIndex::new(ask_api_key))
        .with_nonce(Nonce::new(ask_nonce))
        .sign()
        .await?;

    Ok((
        bid_signed.payload().to_string(),
        ask_signed.payload().to_string(),
    ))
}

/// Sends a signed order payload via WebSocket.
async fn send_signed_order(
    connection: &mut lighter_client::ws_client::WsConnection,
    signed_payload: &str,
) -> Result<()> {
    let payload: Value = serde_json::from_str(signed_payload)?;
    connection.send_transaction(14, payload).await?; // 14 = CreateOrder
    Ok(())
}
