//! Simple limit order example using the modern Rust SDK.
//!
//! Signs and submits a Good-Till-Time limit order via WebSocket, pricing it
//! safely away from the market so it rests on the book.

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::{connect_private_stream, submit_signed_payload, ExampleContext};
use futures_util::StreamExt;
use lighter_client::{
    types::{BaseQty, Expiry, Price},
    ws_client::WsEvent,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("test_simple_limit")).await?;
    let market = ctx.market_id();
    let client = ctx.client();

    // Fetch best bid/ask to anchor limit price
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

    let is_buy = true;
    let distance_pct = 0.05; // 5% away from market so the order rests on the book

    let safe_price = if is_buy {
        best_bid * (1.0 - distance_pct)
    } else {
        best_ask * (1.0 + distance_pct)
    };

    let order_size = 0.005_f64; // 0.005 ETH

    println!("\n{}", "=".repeat(80));
    println!("🧪 Simple LIMIT ORDER (rests on book)");
    println!("{}\n", "=".repeat(80));
    println!("Market: {}", market.into_inner());
    println!("Side:   {}", if is_buy { "BUY" } else { "SELL" });
    println!(
        "Price:  ${:.2} ({}% {} market)",
        safe_price,
        distance_pct * 100.0,
        if is_buy { "below" } else { "above" }
    );
    println!("Size:   {:.4} base units", order_size);

    // Convert to protocol integer formats
    let price_ticks = Price::ticks((safe_price * 100.0).round() as i64);
    let base_qty = BaseQty::try_from((order_size * 10_000.0).round() as i64)
        .expect("size must be greater than zero");

    let expiry = Expiry::from_now(time::Duration::hours(1));
    let client_order_index = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;

    let signer = ctx.signer()?;
    let signed = signer
        .sign_create_order(
            market.into(),
            client_order_index,
            base_qty.into_inner(),
            price_ticks.into_ticks() as i32,
            !is_buy,
            signer.order_type_limit(),
            signer.order_time_in_force_good_till_time(),
            false,
            0,
            expiry.into_unix(),
            None,
            None,
        )
        .await?;

    println!("✍️  Order signed; submitting via WebSocket...\n");

    let builder = client
        .ws()
        .subscribe_transactions()
        .subscribe_executed_transactions();
    let (mut stream, _) = connect_private_stream(&ctx, builder).await?;

    let success = submit_signed_payload(stream.connection_mut(), &signed).await?;
    if success {
        println!("✅ Limit order submitted successfully");
    } else {
        println!("❌ Limit order submission reported a failure");
    }

    println!("\nWaiting for additional WebSocket events (2s)...");
    if let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(2), stream.next()).await {
        match event {
            Ok(WsEvent::Transaction(tx)) => println!("📩 Transaction update: {:?}", tx),
            Ok(WsEvent::ExecutedTransaction(exec)) => {
                println!("📩 Executed transaction: {:?}", exec)
            }
            Ok(other) => common::display_ws_event(other),
            Err(e) => println!("⚠️  WebSocket error: {}", e),
        }
    } else {
        println!("⏳ No additional events within timeout");
    }

    println!("\n✅ Test complete");
    Ok(())
}
