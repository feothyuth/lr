//! Measure latency for signing and submitting a batch of limit orders over the
//! WebSocket API.

mod latency_common;

use anyhow::Result;
use latency_common::{
    aggregate_stats, connect_private_stream, prepare_bench_setup, read_env_usize,
};
use lighter_client::lighter_client::LighterClient;
use lighter_client::signer_client::SignerClient;
use lighter_client::tx_executor::send_batch_tx_ws;
use lighter_client::types::{ApiKeyIndex, Expiry, MarketId, Nonce, Price};
use std::time::Instant;
use time::Duration;

const DEFAULT_BATCH_COUNT: usize = 10;

struct BatchMetrics {
    orders: usize,
    signing_times_ms: Vec<f64>,
    network_ms: f64,
    total_ms: f64,
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

    let order_count = read_env_usize("LATENCY_BATCH_COUNT", DEFAULT_BATCH_COUNT);

    let setup = prepare_bench_setup("benchmark_batch_submit").await?;
    let client = setup.client();
    let signer = setup.signer()?;
    let market = setup.market_id();

    println!("\n{}", "═".repeat(80));
    println!("🔬 Batch Order Latency Benchmark");
    println!("{}", "═".repeat(80));
    println!("Market ID: {}", market.into_inner());
    println!("Price: ${:.2}", setup.price_usd);
    println!("Size: {} base", setup.size_base);
    println!("Batch orders: {}", order_count);
    println!("{}", "═".repeat(80));

    let builder = client
        .ws()
        .subscribe_transactions()
        .subscribe_executed_transactions();
    let (mut stream, _auth_token) = connect_private_stream(&setup.ctx, builder).await?;

    let metrics = benchmark_batch(
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

    println!("Orders: {}", metrics.orders);
    println!("Success: {}", metrics.success);
    let (sign_min, sign_median, sign_p95, sign_max, sign_avg) =
        aggregate_stats(&metrics.signing_times_ms);
    let network_stats = [metrics.network_ms];
    let total_stats = [metrics.total_ms];
    let (net_min, net_median, net_p95, net_max, net_avg) = aggregate_stats(&network_stats);
    let (total_min, total_median, total_p95, total_max, total_avg) = aggregate_stats(&total_stats);
    println!(
        "Signing  -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        sign_min, sign_median, sign_p95, sign_max, sign_avg
    );
    println!(
        "Network  -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        net_min, net_median, net_p95, net_max, net_avg
    );
    println!(
        "Total    -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        total_min, total_median, total_p95, total_max, total_avg
    );

    println!("\n{}", "═".repeat(80));
    println!("⚠ Remember to cancel test orders if necessary.");
    println!("{}", "═".repeat(80));

    Ok(())
}

async fn benchmark_batch(
    stream: &mut lighter_client::ws_client::WsStream,
    client: &LighterClient,
    signer: &SignerClient,
    market: MarketId,
    price_ticks: i32,
    base_qty: lighter_client::types::BaseQty,
    order_count: usize,
) -> Result<BatchMetrics> {
    let mut payloads: Vec<(u8, String)> = Vec::with_capacity(order_count);
    let mut signing_times_ms = Vec::with_capacity(order_count);

    let total_start = Instant::now();

    let (api_key_idx, start_nonce) = signer.next_nonce().await?;

    for idx in 0..order_count {
        let nonce = start_nonce.saturating_add(idx as i64);
        let api_key = ApiKeyIndex::new(api_key_idx);
        let nonce_obj = Nonce::new(nonce);
        let signing_start = Instant::now();
        let signed = client
            .order(market)
            .buy()
            .qty(base_qty)
            .limit(Price::ticks(price_ticks as i64))
            .expires_at(Expiry::from_now(Duration::hours(1)))
            .with_api_key(api_key)
            .with_nonce(nonce_obj)
            .sign()
            .await?;
        let signing_ms = signing_start.elapsed().as_micros() as f64 / 1000.0;
        println!(
            "    signer latency {:>3}/{:>3}: {:.2} ms",
            idx + 1,
            order_count,
            signing_ms
        );
        signing_times_ms.push(signing_ms);
        payloads.push((signed.tx_type() as u8, signed.payload().to_string()));
    }

    let network_start = Instant::now();
    let results = send_batch_tx_ws(stream.connection_mut(), payloads).await?;
    let network_ms = network_start.elapsed().as_micros() as f64 / 1000.0;
    let total_ms = total_start.elapsed().as_micros() as f64 / 1000.0;

    let success = results.iter().filter(|r| **r).count();
    if success != results.len() {
        println!(
            "  ⚠ Batch acks: {} success / {} failure",
            success,
            results.len() - success
        );
    } else {
        println!("  ✓ Batch acknowledged ({} orders)", success);
    }

    Ok(BatchMetrics {
        orders: order_count,
        signing_times_ms,
        network_ms,
        total_ms,
        success,
    })
}
