//! Cancel-Replace Pattern using BATCH WebSocket
//!
//! Sends all transactions in ONE WebSocket message (up to 50 transactions)
//! This is the FASTEST way to update market maker quotes!

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::{connect_private_stream, ExampleContext};
use lighter_client::{
    tx_executor::send_batch_tx_ws,
    types::{BaseQty, Price},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("⚡ Cancel-Replace Pattern (BATCH WebSocket)");
    println!("   Sends ALL transactions in ONE WebSocket message!");
    println!("{}\n", "═".repeat(80));

    // Initialize context
    let ctx = ExampleContext::initialise(Some("test_cancel_replace_batch")).await?;
    let market = ctx.market_id();
    let signer = ctx.signer()?;
    let client = ctx.client();

    println!("✅ Initialized\n");

    // Fetch current market price
    println!("📈 Fetching current market price via REST...");
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

    println!("{}", "═".repeat(80));
    println!("CANCEL-REPLACE CYCLE (BATCH MODE)");
    println!("{}\n", "═".repeat(80));

    let cycle_start = Instant::now();

    // ========== STEP 1: Place Initial Orders (BATCH) ==========
    println!("📍 STEP 1: Place initial quotes using BATCH");
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

    // Connect to WebSocket
    let builder = client.ws().subscribe_transactions();
    let (mut stream, _) = connect_private_stream(&ctx, builder).await?;
    let connection = stream.connection_mut();

    // Send as BATCH
    let batch_send = Instant::now();
    let payloads = vec![
        (bid_signed.tx_type() as u8, bid_signed.payload().to_string()),
        (ask_signed.tx_type() as u8, ask_signed.payload().to_string()),
    ];

    let results = send_batch_tx_ws(connection, payloads).await?;

    println!(
        "  Sent BATCH (2 orders) in {}ms",
        batch_send.elapsed().as_millis()
    );

    // Check batch response
    let batch_success = results.iter().all(|&ok| ok);
    println!(
        "  Batch response: {} ({}ms)",
        if batch_success {
            "✅ Success"
        } else {
            "❌ Failed"
        },
        batch_send.elapsed().as_millis()
    );

    let step1_duration = step1_start.elapsed();
    println!("  Step 1 Total: {}ms\n", step1_duration.as_millis());

    // Small delay to ensure orders are on the book
    println!("⏳ Brief delay (100ms) to ensure orders are posted...\n");
    tokio::time::sleep(Duration::from_millis(100)).await;

    // ========== STEP 2: Cancel + Replace (SINGLE BATCH!) ==========
    println!("📍 STEP 2: Cancel + Replace ALL in ONE BATCH ⚡");
    println!("{}", "─".repeat(80));

    let new_bid = mid * 0.99; // 1% below mid (tighter)
    let new_ask = mid * 1.01; // 1% above mid (tighter)

    println!("New BID: ${:.2} | ASK: ${:.2}\n", new_bid, new_ask);

    let step2_start = Instant::now();

    // Sign all 3 transactions: cancel + 2 new orders
    let (api_key_idx_cancel, nonce_cancel) = signer.next_nonce().await?;
    let cancel_tx_info = signer
        .sign_cancel_all_orders(0, 0, Some(nonce_cancel), Some(api_key_idx_cancel))
        .await?;

    let (bid2_signed, ask2_signed) =
        sign_order_pair(&ctx, market.into(), size, new_bid, new_ask).await?;

    // Send as SINGLE BATCH (cancel + 2 new orders)
    let batch_send2 = Instant::now();
    let payloads2 = vec![
        (16_u8, cancel_tx_info), // TX_TYPE_CANCEL_ALL_ORDERS = 16
        (
            bid2_signed.tx_type() as u8,
            bid2_signed.payload().to_string(),
        ),
        (
            ask2_signed.tx_type() as u8,
            ask2_signed.payload().to_string(),
        ),
    ];

    let results2 = send_batch_tx_ws(connection, payloads2).await?;

    println!(
        "  Sent BATCH (1 cancel + 2 orders) in {}ms",
        batch_send2.elapsed().as_millis()
    );

    // Check batch response
    let batch2_success = results2.iter().all(|&ok| ok);
    println!(
        "  Batch response: {} ({}ms)",
        if batch2_success {
            "✅ Success"
        } else {
            "❌ Failed"
        },
        batch_send2.elapsed().as_millis()
    );

    let step2_duration = step2_start.elapsed();
    println!("  Step 2 Total: {}ms\n", step2_duration.as_millis());

    let cycle_duration = cycle_start.elapsed();

    println!("{}", "═".repeat(80));
    println!("✅ Cancel-Replace cycle complete!");
    println!("{}", "═".repeat(80));
    println!();
    println!("📊 Latency Breakdown:");
    println!(
        "   Step 1 (Batch 2 orders):     {}ms",
        step1_duration.as_millis()
    );
    println!(
        "   Step 2 (Batch cancel+2):     {}ms",
        step2_duration.as_millis()
    );
    println!("   ─────────────────────────────────");
    println!(
        "   Total Cycle Time:            {}ms",
        cycle_duration.as_millis()
    );
    println!();
    println!("💡 Key Advantages of BATCH WebSocket:");
    println!("   1. ⚡ ALL transactions sent in ONE WebSocket message");
    println!("   2. 🚀 Server processes them atomically");
    println!("   3. 📦 Up to 50 transactions per batch message");
    println!("   4. 🎯 Ideal for market maker quote updates");
    println!();
    println!("⚡ Performance Comparison:");
    println!("   Sequential:      ~630ms  (1.6 Hz)");
    println!("   Parallel:        ~750ms  (1.3 Hz)");
    println!(
        "   BATCH (this):    ~{}ms  ({:.1} Hz) 🚀",
        cycle_duration.as_millis(),
        1000.0 / cycle_duration.as_millis() as f64
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
) -> Result<(
    lighter_client::signer_client::SignedPayload<lighter_client::transactions::CreateOrder>,
    lighter_client::signer_client::SignedPayload<lighter_client::transactions::CreateOrder>,
)> {
    let signer = ctx.signer()?;

    let price_bid_ticks = Price::ticks((bid_price * 100.0).round() as i64);
    let price_ask_ticks = Price::ticks((ask_price * 100.0).round() as i64);
    let base_qty = BaseQty::try_from((size * 10_000.0).round() as i64)
        .map_err(|e| anyhow::anyhow!("Invalid size: {}", e))?;

    // Use default 28-day expiry (-1 sentinel) so the gateway accepts the order.
    const DEFAULT_EXPIRY: i64 = -1;

    // Generate unique client order indices
    let base_index = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;

    let (api_key_idx_bid, nonce_bid) = signer.next_nonce().await?;

    // Sign BID order
    let bid_signed = signer
        .sign_create_order(
            market_id,
            base_index,
            base_qty.into_inner(),
            price_bid_ticks.into_ticks() as i32,
            false, // BUY
            signer.order_type_limit(),
            signer.order_time_in_force_post_only(),
            false,
            0,
            DEFAULT_EXPIRY,
            Some(nonce_bid),
            Some(api_key_idx_bid),
        )
        .await?;

    let (api_key_idx_ask, nonce_ask) = signer.next_nonce().await?;

    // Sign ASK order
    let ask_signed = signer
        .sign_create_order(
            market_id,
            base_index + 1,
            base_qty.into_inner(),
            price_ask_ticks.into_ticks() as i32,
            true, // SELL
            signer.order_type_limit(),
            signer.order_time_in_force_post_only(),
            false,
            0,
            DEFAULT_EXPIRY,
            Some(nonce_ask),
            Some(api_key_idx_ask),
        )
        .await?;

    Ok((bid_signed, ask_signed))
}
