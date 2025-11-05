//! Market order example using the Lighter Rust SDK.
//!
//! Submits a market order (IOC) via WebSocket to demonstrate fast execution.

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::{connect_private_stream, submit_signed_payload, ExampleContext};
use futures_util::StreamExt;
use lighter_client::ws_client::WsEvent;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("test_market_order")).await?;
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

    let is_buy = false; // market sell by default (change to true for buy)
    let slippage_pct = 0.01; // 1% buffer to avoid rejection
    let slippage_price = if is_buy {
        best_ask * (1.0 + slippage_pct)
    } else {
        best_bid * (1.0 - slippage_pct)
    };

    let order_size = 0.005_f64; // 0.005 ETH

    println!("\n{}", "=".repeat(80));
    println!("⚡ Market ORDER (IOC)");
    println!("{}\n", "=".repeat(80));
    println!("Market: {}", market.into_inner());
    println!("Side:   {}", if is_buy { "BUY" } else { "SELL" });
    println!("Slippage reference price: ${:.2}", slippage_price);
    println!("Size:   {:.4} base units", order_size);
    println!("Behaviour: Immediate-Or-Cancel");

    let price_ticks = ((slippage_price * 100.0).round() as i64).max(1) as i32;
    let base_units = (order_size * 10_000.0).round() as i64;
    let base_qty = lighter_client::types::BaseQty::try_from(base_units)
        .expect("size must be greater than zero");

    let client_order_index = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;

    let signer = ctx.signer()?;
    let signed = signer
        .sign_create_order(
            market.into(),
            client_order_index,
            base_qty.into_inner(),
            price_ticks,
            !is_buy,
            signer.order_type_market(),
            signer.order_time_in_force_immediate_or_cancel(),
            false,
            0,
            0,
            None,
            None,
        )
        .await?;

    println!("✍️  Market order signed; submitting via WebSocket...\n");

    let builder = client
        .ws()
        .subscribe_transactions()
        .subscribe_executed_transactions();
    let (mut stream, _) = connect_private_stream(&ctx, builder).await?;

    let success = submit_signed_payload(stream.connection_mut(), &signed).await?;
    if success {
        println!("✅ Market order submitted successfully");
    } else {
        println!("❌ Market order submission reported a failure");
    }

    println!("\nMonitoring for fills (2s)...");
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
