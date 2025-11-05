//! Post-only limit order example using the Lighter Rust SDK.
//!
//! Places a maker-only order (PostOnly time-in-force) via WebSocket, pricing it
//! safely away from the market to guarantee it rests on the book.

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::{connect_private_stream, submit_signed_payload, ExampleContext};
use futures_util::StreamExt;
use lighter_client::{
    types::{Expiry, Price},
    ws_client::WsEvent,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("test_postonly_order")).await?;
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

    let is_buy = true;
    let distance_pct = 0.05; // 5% away ensures maker-only behaviour
    let safe_price = if is_buy {
        best_bid * (1.0 - distance_pct)
    } else {
        best_ask * (1.0 + distance_pct)
    };

    let order_size = 0.005_f64; // 0.005 ETH minimum

    println!("\n{}", "=".repeat(80));
    println!("🧪 Post-Only LIMIT ORDER");
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
    println!("Time-In-Force: PostOnly (maker only)");

    let price_ticks = Price::ticks((safe_price * 100.0).round() as i64);
    let base_units = (order_size * 10_000.0).round() as i64;
    let base_qty = lighter_client::types::BaseQty::try_from(base_units)
        .expect("size must be greater than zero");

    let expiry = Expiry::from_now(time::Duration::minutes(15)); // exchange requires >=10min
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
            signer.order_time_in_force_post_only(),
            false,
            0,
            expiry.into_unix(),
            None,
            None,
        )
        .await?;

    println!("✍️  Post-only order signed; submitting via WebSocket...\n");

    let builder = client
        .ws()
        .subscribe_transactions()
        .subscribe_executed_transactions();
    let (mut stream, _) = connect_private_stream(&ctx, builder).await?;

    let success = submit_signed_payload(stream.connection_mut(), &signed).await?;
    if success {
        println!("✅ Post-only limit order submitted successfully");
    } else {
        println!("❌ Order submission reported a failure");
    }

    println!("\nMonitoring for follow-up events (2s)...");
    if let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(2), stream.next()).await {
        match event {
            Ok(WsEvent::Transaction(tx)) => println!("📩 Transaction update: {:?}", tx),
            Ok(other) => common::display_ws_event(other),
            Err(e) => println!("⚠️  WebSocket error: {}", e),
        }
    }

    println!("\n✅ Test complete");
    Ok(())
}
