//! Optimised batch order latency benchmark.
//!
//! Adds signer warm-up, optional per-signature logging, and side selection so
//! the run focuses on steady-state performance of the WebSocket batch path.

mod latency_common;

use anyhow::Result;
use latency_common::{
    aggregate_stats, connect_private_stream, fetch_next_nonce, prepare_bench_setup, read_env_bool,
    read_env_usize, sign_limit_order,
};
use lighter_client::tx_executor::send_batch_tx_ws;
use lighter_client::types::Expiry;
use std::time::Instant;
use time::Duration;

const DEFAULT_BATCH_COUNT: usize = 10;
const DEFAULT_WARMUP_SIGNATURES: usize = 1;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lighter_client=info".into()),
        )
        .try_init();

    let order_count = read_env_usize("LATENCY_BATCH_COUNT", DEFAULT_BATCH_COUNT);
    let warmup_signatures = read_env_usize("LATENCY_WARMUP_SIGNATURES", DEFAULT_WARMUP_SIGNATURES);
    let log_signer = read_env_bool("LATENCY_LOG_SIGNER", false);
    let side_is_ask = read_env_bool("LATENCY_SIDE_IS_ASK", false);
    let is_bid = !side_is_ask;

    let setup = prepare_bench_setup("benchmark_batch_submit_v2").await?;
    let client = setup.client();
    let _ = setup.signer()?; // ensure signer is available
    let market = setup.market_id();
    let api_key = setup.ctx.api_key_index();

    println!("\n{}", "═".repeat(80));
    println!("🚀 Batch Order Latency Benchmark v2");
    println!("{}", "═".repeat(80));
    println!("Market ID: {}", market.into_inner());
    println!("Price: ${:.2}", setup.price_usd);
    println!("Size: {} base", setup.size_base);
    println!("Side: {}", if is_bid { "bid (buy)" } else { "ask (sell)" });
    println!("Batch orders: {}", order_count);
    println!(
        "Signer warm-up: {} signature{}",
        warmup_signatures,
        if warmup_signatures == 1 { "" } else { "s" }
    );
    println!("{}", "═".repeat(80));

    // Prime signer + WebSocket connection.
    let builder = client
        .ws()
        .subscribe_transactions()
        .subscribe_executed_transactions();
    let (mut stream, _auth_token) = connect_private_stream(&setup.ctx, builder).await?;

    let expiry = Expiry::from_now(Duration::hours(1));
    let mut bench_start_nonce = fetch_next_nonce(client, api_key).await?;

    if warmup_signatures > 0 {
        println!("Warm-up (sign only, not submitted):");
        let mut warmup_stats = Vec::with_capacity(warmup_signatures);
        for idx in 0..warmup_signatures {
            let (_, signing_ms) = sign_limit_order(
                client,
                market,
                setup.base_qty,
                setup.price_ticks,
                is_bid,
                api_key,
                bench_start_nonce + idx as i64,
                expiry,
            )
            .await?;
            warmup_stats.push(signing_ms);
            if log_signer {
                println!(
                    "    warm-up signer {:>3}/{:>3}: {:.2} ms",
                    idx + 1,
                    warmup_signatures,
                    signing_ms
                );
            }
        }
        let (min, median, p95, max, avg) = aggregate_stats(&warmup_stats);
        println!(
            "  Warm-up signing -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
            min, median, p95, max, avg
        );
        println!("{}", "─".repeat(80));
    }

    bench_start_nonce = fetch_next_nonce(client, api_key).await?;
    let mut payloads: Vec<(u8, String)> = Vec::with_capacity(order_count);
    let mut signing_ms = Vec::with_capacity(order_count);

    let total_start = Instant::now();

    for idx in 0..order_count {
        let (signed, sign_ms) = sign_limit_order(
            client,
            market,
            setup.base_qty,
            setup.price_ticks,
            is_bid,
            api_key,
            bench_start_nonce + idx as i64,
            expiry,
        )
        .await?;
        if log_signer {
            println!(
                "    signer latency {:>3}/{:>3}: {:.2} ms",
                idx + 1,
                order_count,
                sign_ms
            );
        }
        signing_ms.push(sign_ms);
        let (entry, _) = signed.into_parts();
        payloads.push((entry.tx_type as u8, entry.tx_info));
    }

    let network_start = Instant::now();
    let results = send_batch_tx_ws(stream.connection_mut(), payloads).await?;
    let network_ms = network_start.elapsed().as_secs_f64() * 1_000.0;
    let total_ms = total_start.elapsed().as_secs_f64() * 1_000.0;

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

    println!("\n{}", "═".repeat(80));
    println!("Summary");
    println!("{}", "═".repeat(80));
    println!("Orders: {}", order_count);
    println!("Success: {}", success);

    let (sign_min, sign_median, sign_p95, sign_max, sign_avg) = aggregate_stats(&signing_ms);
    println!(
        "Signing  -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        sign_min, sign_median, sign_p95, sign_max, sign_avg
    );

    let network_stats = vec![network_ms];
    let (net_min, net_median, net_p95, net_max, net_avg) = aggregate_stats(&network_stats);
    println!(
        "Network  -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        net_min, net_median, net_p95, net_max, net_avg
    );

    let total_stats = vec![total_ms];
    let (total_min, total_median, total_p95, total_max, total_avg) = aggregate_stats(&total_stats);
    println!(
        "Total    -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        total_min, total_median, total_p95, total_max, total_avg
    );

    println!("\n{}", "═".repeat(80));
    println!("⚠ Remember to cancel test orders if necessary.");
    println!("{}", "═".repeat(80));

    Ok(())
}
