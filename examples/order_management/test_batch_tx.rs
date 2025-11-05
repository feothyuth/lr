//! Batch transaction submission using the Lighter Rust SDK.
//!
//! Signs multiple limit orders and submits them as a single WebSocket batch,
//! demonstrating the SDK’s nonce management and batch helpers.

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::{connect_private_stream, ExampleContext};
use futures_util::StreamExt;
use lighter_client::{
    tx_executor::send_batch_tx_ws,
    types::{BaseQty, Expiry, Price},
    ws_client::WsEvent,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("test_batch_tx")).await?;
    let market = ctx.market_id();
    let client = ctx.client();
    let signer = ctx.signer()?;

    const BATCH_SIZE: usize = 2;
    let order_book = client.orders().book(market, 25).await?;
    let (best_bid, _) = order_book
        .bids
        .first()
        .map(|bid| bid.price.parse::<f64>().unwrap_or_default())
        .zip(
            order_book
                .asks
                .first()
                .map(|ask| ask.price.parse::<f64>().unwrap_or_default()),
        )
        .unwrap_or((0.0, 0.0));

    let price_levels = [0.80, 0.75]; // 20% and 25% below bid
    let order_size = 0.005_f64; // 0.005 ETH per order

    println!("\n{}", "=".repeat(80));
    println!(
        "🧪 Batch Transaction Submission ({} orders, {:.3} base units each)",
        BATCH_SIZE, order_size
    );
    println!("{}\n", "=".repeat(80));

    let mut signed_orders = Vec::with_capacity(BATCH_SIZE);
    for (idx, level) in price_levels.iter().take(BATCH_SIZE).enumerate() {
        let price = best_bid * level;
        let price_ticks = Price::ticks((price * 100.0).round() as i64);
        let base_qty = BaseQty::try_from((order_size * 10_000.0).round() as i64)
            .expect("size must be greater than zero");

        let expiry = Expiry::from_now(time::Duration::minutes(15));
        let client_order_index =
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64 + idx as i64;

        let signed = signer
            .sign_create_order(
                market.into(),
                client_order_index,
                base_qty.into_inner(),
                price_ticks.into_ticks() as i32,
                false, // BUY
                signer.order_type_limit(),
                signer.order_time_in_force_good_till_time(),
                false,
                0,
                expiry.into_unix(),
                None,
                None,
            )
            .await?;

        println!(
            "  [{}] BUY {:.3} @ ${:.2} (ticks: {})",
            idx + 1,
            order_size,
            price,
            price_ticks.into_ticks()
        );

        signed_orders.push(signed);
    }

    let builder = client
        .ws()
        .subscribe_transactions()
        .subscribe_executed_transactions();
    let (mut stream, _) = connect_private_stream(&ctx, builder).await?;
    let mut connection = stream.connection_mut();

    let payloads = signed_orders
        .iter()
        .map(|signed| (signed.tx_type() as u8, signed.payload().to_string()))
        .collect::<Vec<_>>();

    println!("\n📤 Sending batch...");
    let results = send_batch_tx_ws(&mut connection, payloads).await?;

    println!("\n{}", "=".repeat(80));
    println!("📊 Batch Results:");
    for (idx, ok) in results.iter().enumerate() {
        let status = if *ok { "✅ ACCEPTED" } else { "❌ FAILED" };
        println!("  Order {}: {}", idx + 1, status);
    }

    println!("\nMonitoring for follow-up events (2s)...");
    if let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(2), stream.next()).await {
        match event {
            Ok(WsEvent::Transaction(tx)) => println!("📩 Transaction event: {:?}", tx),
            Ok(WsEvent::ExecutedTransaction(exec)) => {
                println!("📩 Executed transaction: {:?}", exec)
            }
            Ok(other) => common::display_ws_event(other),
            Err(err) => println!("⚠️  WebSocket error: {}", err),
        }
    }

    println!("\n✅ Test complete");
    Ok(())
}
