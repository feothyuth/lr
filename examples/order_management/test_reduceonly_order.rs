//! Reduce-only limit order example using the Lighter Rust SDK.
//!
//! Demonstrates how to place a reduce-only order to safely close an existing
//! position without increasing exposure.

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::{connect_private_stream, submit_signed_payload, ExampleContext};
use futures_util::StreamExt;
use lighter_client::ws_client::WsEvent;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("test_reduceonly_order")).await?;
    let market = ctx.market_id();
    let client = ctx.client();

    let order_book = client.orders().book(market, 25).await?;
    let (best_bid, best_ask) = order_book
        .bids
        .first()
        .and_then(|bid| order_book.asks.first().map(|ask| (bid, ask)))
        .map(|(bid, ask)| {
            (
                bid.price.parse::<f64>().unwrap_or_default(),
                ask.price.parse::<f64>().unwrap_or_default(),
            )
        })
        .unwrap_or((0.0, 0.0));

    let closing_long = true; // SELL reduce-only by default
    let price_buffer_pct = 0.02; // 2% inside market to ensure execution
    let price = if closing_long {
        best_bid * (1.0 - price_buffer_pct)
    } else {
        best_ask * (1.0 + price_buffer_pct)
    };

    let order_size = 0.005_f64; // 0.005 ETH

    println!("\n{}", "=".repeat(80));
    println!("🔒 Reduce-Only LIMIT ORDER");
    println!("{}\n", "=".repeat(80));
    println!("Market: {}", market.into_inner());
    println!("Closing: {}", if closing_long { "LONG" } else { "SHORT" });
    println!(
        "Price:  ${:.2} ({}% from market)",
        price,
        price_buffer_pct * 100.0
    );
    println!("Size:   {:.4} base units", order_size);

    let price_ticks = ((price * 100.0).round() as i64).max(1) as i32;
    let base_units = (order_size * 10_000.0).round() as i64;
    let base_qty = lighter_client::types::BaseQty::try_from(base_units)
        .expect("size must be greater than zero");

    let expiry = lighter_client::types::Expiry::from_now(time::Duration::minutes(15));
    let client_order_index = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;

    let signer = ctx.signer()?;
    let signed = signer
        .sign_create_order(
            market.into(),
            client_order_index,
            base_qty.into_inner(),
            price_ticks,
            closing_long, // SELL to close long (is_ask)
            signer.order_type_limit(),
            signer.order_time_in_force_good_till_time(),
            true, // reduce_only
            0,
            expiry.into_unix(),
            None,
            None,
        )
        .await?;

    println!("✍️  Reduce-only order signed; submitting via WebSocket...\n");

    let builder = client
        .ws()
        .subscribe_transactions()
        .subscribe_executed_transactions();
    let (mut stream, _) = connect_private_stream(&ctx, builder).await?;

    let success = submit_signed_payload(stream.connection_mut(), &signed).await?;
    if success {
        println!("✅ Reduce-only order submitted successfully");
    } else {
        println!("❌ Reduce-only order submission reported a failure");
    }

    println!("\nMonitoring for responses (2s)...");
    if let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(2), stream.next()).await {
        match event {
            Ok(WsEvent::ExecutedTransaction(exec)) => {
                println!("📩 Executed transaction: {:?}", exec)
            }
            Ok(other) => common::display_ws_event(other),
            Err(e) => println!("⚠️  WebSocket error: {}", e),
        }
    }

    println!("\n✅ Test complete");
    Ok(())
}
