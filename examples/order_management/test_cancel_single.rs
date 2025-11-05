//! Cancel a single order via WebSocket using the Lighter Rust SDK.
//!
//! Provide the market ID and order index to cancel. The order index can be
//! obtained from the `monitor_active_orders` example.

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
    let ctx = ExampleContext::initialise(Some("test_cancel_single")).await?;
    let signer = ctx.signer()?;
    let client = ctx.client();

    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: cargo run --example test_cancel_single <market_id> <order_index>");
        return Ok(());
    }

    let market_id: i32 = args[1]
        .parse()
        .context("Invalid market_id (must be integer)")?;
    let order_index: i64 = args[2]
        .parse()
        .context("Invalid order_index (must be integer)")?;

    println!("\n{}", "═".repeat(80));
    println!("🗑️  Cancel Single Order");
    println!("{}\n", "═".repeat(80));
    println!("Market ID:   {}", market_id);
    println!("Order Index: {}\n", order_index);

    let sign_start = Instant::now();
    let signed = signer
        .sign_cancel_order(market_id, order_index, None, None)
        .await?;
    let sign_duration = sign_start.elapsed();

    println!(
        "✍️  Cancel transaction signed ({}ms)",
        sign_duration.as_millis()
    );

    let builder = client.ws().subscribe_transactions();
    let (mut stream, _) = connect_private_stream(&ctx, builder).await?;

    let entry = signed.as_batch_entry();
    let payload: Value =
        serde_json::from_str(&entry.tx_info).context("signed payload is not valid JSON")?;

    let connection = stream.connection_mut();
    connection
        .send_transaction(entry.tx_type as u8, payload)
        .await?;

    println!("📤 Transaction sent; awaiting confirmation...");
    let response_start = Instant::now();

    let outcome = timeout(
        Duration::from_secs(5),
        connection.wait_for_tx_response(Duration::from_secs(5)),
    )
    .await;

    match outcome {
        Ok(Ok(true)) => {
            println!(
                "✅ Order cancelled successfully ({}ms)",
                response_start.elapsed().as_millis()
            );
        }
        Ok(Ok(false)) => {
            println!(
                "❌ Cancel rejected ({}ms) — order may not exist or is already filled",
                response_start.elapsed().as_millis()
            );
        }
        Ok(Err(err)) => {
            println!("⚠️  WebSocket error while waiting for response: {}", err);
        }
        Err(_) => {
            println!("⏳ Timed out waiting for cancel acknowledgement");
        }
    }

    // Drain any remaining events briefly to surface additional context.
    if let Ok(Some(event)) = timeout(Duration::from_secs(1), stream.next()).await {
        match event {
            Ok(WsEvent::Transaction(tx)) => println!("📩 Extra transaction event: {:?}", tx),
            Ok(other) => common::display_ws_event(other),
            Err(err) => println!("⚠️  WebSocket error: {}", err),
        }
    }

    println!("\n✅ Finished");
    Ok(())
}
