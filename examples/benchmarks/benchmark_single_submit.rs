//! Measure latency for signing and submitting individual limit orders over the
//! WebSocket API.

mod latency_common;

use anyhow::Result;
use latency_common::example_context::submit_signed_payload;
use latency_common::{
    aggregate_stats, connect_private_stream, prepare_bench_setup, read_env_usize,
};
use lighter_client::lighter_client::LighterClient;
use lighter_client::signer_client::SignerClient;
use lighter_client::types::{ApiKeyIndex, Expiry, MarketId, Nonce, Price};
use std::time::{Duration as StdDuration, Instant};
use time::Duration;

const DEFAULT_SINGLE_COUNT: usize = 10;

struct SingleMetrics {
    orders: usize,
    signing_ms: Vec<f64>,
    network_ms: Vec<f64>,
    total_ms: Vec<f64>,
    success: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lighter_client=info".into()),
        )
        .try_init();

    let order_count = read_env_usize("LATENCY_SINGLE_COUNT", DEFAULT_SINGLE_COUNT);

    let setup = prepare_bench_setup("benchmark_single_submit").await?;
    let client = setup.client();
    let signer = setup.signer()?;
    let market = setup.market_id();

    println!("\n{}", "═".repeat(80));
    println!("🔬 Single Order Latency Benchmark");
    println!("{}", "═".repeat(80));
    println!("Market ID: {}", market.into_inner());
    println!("Price: ${:.2}", setup.price_usd);
    println!("Size: {} base", setup.size_base);
    println!("Orders: {}", order_count);
    println!("{}", "═".repeat(80));

    let builder = client
        .ws()
        .subscribe_transactions()
        .subscribe_executed_transactions();
    let (mut stream, _auth_token) = connect_private_stream(&setup.ctx, builder).await?;

    let metrics = benchmark_single(
        &mut stream,
        client,
        signer,
        market,
        setup.price_ticks as i32,
        setup.base_qty,
        order_count,
    )
    .await?;

    println!("\n{}", "═".repeat(80));
    println!("Summary");
    println!("{}", "═".repeat(80));

    let (single_sign_min, single_sign_median, single_sign_p95, single_sign_max, single_sign_avg) =
        aggregate_stats(&metrics.signing_ms);
    let (single_net_min, single_net_median, single_net_p95, single_net_max, single_net_avg) =
        aggregate_stats(&metrics.network_ms);
    let (
        single_total_min,
        single_total_median,
        single_total_p95,
        single_total_max,
        single_total_avg,
    ) = aggregate_stats(&metrics.total_ms);

    println!("Orders: {}", metrics.orders);
    println!("Success: {}", metrics.success);
    println!(
        "Signing  -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        single_sign_min, single_sign_median, single_sign_p95, single_sign_max, single_sign_avg
    );
    println!(
        "Network  -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        single_net_min, single_net_median, single_net_p95, single_net_max, single_net_avg
    );
    println!(
        "Total    -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        single_total_min, single_total_median, single_total_p95, single_total_max, single_total_avg
    );

    println!("\n{}", "═".repeat(80));
    println!("⚠ Remember to cancel test orders if necessary.");
    println!("{}", "═".repeat(80));

    Ok(())
}

async fn benchmark_single(
    stream: &mut lighter_client::ws_client::WsStream,
    client: &LighterClient,
    signer: &SignerClient,
    market: MarketId,
    price_ticks: i32,
    base_qty: lighter_client::types::BaseQty,
    order_count: usize,
) -> Result<SingleMetrics> {
    let mut signing_ms = Vec::with_capacity(order_count);
    let mut network_ms = Vec::with_capacity(order_count);
    let mut total_ms = Vec::with_capacity(order_count);
    let mut success_count = 0usize;

    let (api_key_idx, start_nonce) = signer.next_nonce().await?;
    let api_key = ApiKeyIndex::new(api_key_idx);

    for idx in 0..order_count {
        let total_start = Instant::now();
        let signing_start = Instant::now();
        let nonce = Nonce::new(start_nonce.saturating_add(idx as i64));
        let signed = client
            .order(market)
            .buy()
            .qty(base_qty)
            .limit(Price::ticks(price_ticks as i64))
            .with_api_key(api_key)
            .with_nonce(nonce)
            .expires_at(Expiry::from_now(Duration::hours(1)))
            .sign()
            .await?;
        let signing_elapsed = signing_start.elapsed().as_micros() as f64 / 1000.0;
        println!(
            "    signer latency {:>3}/{:>3}: {:.2} ms",
            idx + 1,
            order_count,
            signing_elapsed
        );

        let network_start = Instant::now();
        let success = submit_signed_payload(stream.connection_mut(), &signed).await?;
        let network_elapsed = network_start.elapsed().as_micros() as f64 / 1000.0;
        let total_elapsed = total_start.elapsed().as_micros() as f64 / 1000.0;

        if success {
            success_count += 1;
        }

        println!(
            "  Order {:>3}/{:>3}: total {:.2} ms (sign {:.2} ms | net {:.2} ms) {}",
            idx + 1,
            order_count,
            total_elapsed,
            signing_elapsed,
            network_elapsed,
            if success { "✓" } else { "✗" }
        );

        signing_ms.push(signing_elapsed);
        network_ms.push(network_elapsed);
        total_ms.push(total_elapsed);

        tokio::time::sleep(StdDuration::from_millis(50)).await;
    }

    Ok(SingleMetrics {
        orders: order_count,
        signing_ms,
        network_ms,
        total_ms,
        success: success_count,
    })
}
