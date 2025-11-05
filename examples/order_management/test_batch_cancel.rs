//! Batch cancel multiple orders via WebSocket using the Rust SDK.
//!
//! Cancels up to 50 orders in a single run. Provide the market ID followed by
//! one or more order indices (as reported by `monitor_active_orders`).

#[path = "../common/example_context.rs"]
mod common;

use anyhow::{Context, Result};
use common::{connect_private_stream, ExampleContext};
use futures_util::StreamExt;
use lighter_client::ws_client::WsEvent;
use serde_json::Value;
use std::{env, time::Instant};
use tokio::time::{timeout, Duration};

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("test_batch_cancel")).await?;
    let signer = ctx.signer()?;
    let client = ctx.client();

    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: cargo run --example test_batch_cancel <market_id> <order_index1> <order_index2> ...");
        eprintln!("(Cancels up to 50 orders in a single batch)");
        return Ok(());
    }

    let market_id: i32 = args[1]
        .parse()
        .context("First argument must be the market_id (e.g., 0 for ETH-PERP)")?;
    let order_indices: Vec<i64> = args[2..]
        .iter()
        .map(|value| value.parse::<i64>())
        .collect::<Result<Vec<_>, _>>()
        .context("Order indices must be integers")?;

    if order_indices.is_empty() {
        eprintln!("❌ Provide at least one order index to cancel.");
        return Ok(());
    }
    if order_indices.len() > 50 {
        eprintln!("❌ Too many orders. Maximum batch size is 50.");
        return Ok(());
    }

    println!("\n{}", "═".repeat(80));
    println!("🗑️  Batch Cancel Orders");
    println!("{}\n", "═".repeat(80));
    println!("Market ID: {}", market_id);
    println!("Order indices: {:?}\n", order_indices);

    let mut signed_cancels = Vec::with_capacity(order_indices.len());
    let sign_start = Instant::now();
    for &order_index in &order_indices {
        let signed = signer
            .sign_cancel_order(market_id, order_index, None, None)
            .await?;
        signed_cancels.push(signed);
    }
    let sign_duration = sign_start.elapsed();
    println!(
        "✍️  Signed {} cancel transactions ({} ms)\n",
        signed_cancels.len(),
        sign_duration.as_millis()
    );

    let builder = client.ws().subscribe_transactions();
    let (mut stream, _) = connect_private_stream(&ctx, builder).await?;
    let connection = stream.connection_mut();

    let send_start = Instant::now();
    for signed in &signed_cancels {
        let entry = signed.as_batch_entry();
        let payload: Value = serde_json::from_str(&entry.tx_info)
            .context("Signed cancel payload is not valid JSON")?;
        connection
            .send_transaction(entry.tx_type as u8, payload)
            .await?;
    }
    let send_duration = send_start.elapsed();
    println!(
        "📤 Submitted {} cancel transactions ({} ms)\n",
        signed_cancels.len(),
        send_duration.as_millis()
    );

    println!("⏳ Waiting for responses...\n");
    let response_start = Instant::now();
    let mut success = 0;
    let mut rejected = 0;

    for (idx, _signed) in signed_cancels.iter().enumerate() {
        match timeout(
            Duration::from_secs(5),
            connection.wait_for_tx_response(Duration::from_secs(5)),
        )
        .await
        {
            Ok(Ok(true)) => {
                success += 1;
                println!("  ✅ Order {} cancelled", idx + 1);
            }
            Ok(Ok(false)) => {
                rejected += 1;
                println!("  ❌ Order {} rejected", idx + 1);
            }
            Ok(Err(err)) => {
                println!("  ⚠️  WebSocket error for order {}: {}", idx + 1, err);
            }
            Err(_) => {
                println!("  ⏳ Order {} timeout", idx + 1);
            }
        }
    }

    let response_duration = response_start.elapsed();

    if let Ok(Some(event)) = timeout(Duration::from_secs(1), stream.next()).await {
        match event {
            Ok(WsEvent::Transaction(tx)) => println!("📩 Extra transaction event: {:?}", tx),
            Ok(other) => common::display_ws_event(other),
            Err(err) => println!("⚠️  WebSocket error: {}", err),
        }
    }

    println!("\n📊 Summary");
    println!("   Success:  {}", success);
    println!("   Rejected: {}", rejected);
    println!(
        "   Signing:  {} ms | Sending: {} ms | Responses: {} ms",
        sign_duration.as_millis(),
        send_duration.as_millis(),
        response_duration.as_millis()
    );

    println!("\n✅ Test complete");
    Ok(())
}
