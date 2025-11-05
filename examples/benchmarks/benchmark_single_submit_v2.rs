//! Optimised single-order latency benchmark.
//!
//! Adds signer warm-up, configurable pacing, and optional per-order logging to
//! study steady-state WebSocket latency when orders are submitted one by one.

mod latency_common;

use anyhow::Result;
use latency_common::example_context::submit_signed_payload;
use latency_common::{
    aggregate_stats, connect_private_stream, fetch_next_nonce, prepare_bench_setup, read_env_bool,
    read_env_usize, sign_limit_order,
};
use lighter_client::types::Expiry;
use std::time::{Duration as StdDuration, Instant};
use time::Duration;

const DEFAULT_SINGLE_COUNT: usize = 10;
const DEFAULT_WARMUP_SIGNATURES: usize = 1;
const DEFAULT_PAUSE_MS: usize = 0;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lighter_client=info".into()),
        )
        .try_init();

    let order_count = read_env_usize("LATENCY_SINGLE_COUNT", DEFAULT_SINGLE_COUNT);
    let warmup_signatures = read_env_usize("LATENCY_WARMUP_SIGNATURES", DEFAULT_WARMUP_SIGNATURES);
    let pause_ms = read_env_usize("LATENCY_PAUSE_MS", DEFAULT_PAUSE_MS);
    let log_signer = read_env_bool("LATENCY_LOG_SIGNER", false);
    let log_each_order = read_env_bool("LATENCY_LOG_EACH_ORDER", false);
    let side_is_ask = read_env_bool("LATENCY_SIDE_IS_ASK", false);
    let is_bid = !side_is_ask;

    let setup = prepare_bench_setup("benchmark_single_submit_v2").await?;
    let client = setup.client();
    let _ = setup.signer()?; // ensure signer is available
    let market = setup.market_id();
    let api_key = setup.ctx.api_key_index();

    println!("\n{}", "═".repeat(80));
    println!("⚡ Single Order Latency Benchmark v2");
    println!("{}", "═".repeat(80));
    println!("Market ID: {}", market.into_inner());
    println!("Price: ${:.2}", setup.price_usd);
    println!("Size: {} base", setup.size_base);
    println!("Side: {}", if is_bid { "bid (buy)" } else { "ask (sell)" });
    println!("Orders: {}", order_count);
    println!(
        "Signer warm-up: {} signature{}",
        warmup_signatures,
        if warmup_signatures == 1 { "" } else { "s" }
    );
    println!(
        "Pause between orders: {} ms",
        if pause_ms > 0 { pause_ms as i64 } else { 0 }
    );
    println!("{}", "═".repeat(80));

    let pause_duration = if pause_ms > 0 {
        Some(StdDuration::from_millis(pause_ms as u64))
    } else {
        None
    };

    let builder = client
        .ws()
        .subscribe_transactions()
        .subscribe_executed_transactions();
    let (mut stream, _auth_token) = connect_private_stream(&setup.ctx, builder).await?;

    let expiry = Expiry::from_now(Duration::hours(1));
    let mut bench_start_nonce = fetch_next_nonce(client, api_key).await?;

    if warmup_signatures > 0 {
        println!("Warm-up (sign only, not submitted):");
        let mut warm_stats = Vec::with_capacity(warmup_signatures);
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
            warm_stats.push(signing_ms);
            if log_signer {
                println!(
                    "    warm-up signer {:>3}/{:>3}: {:.2} ms",
                    idx + 1,
                    warmup_signatures,
                    signing_ms
                );
            }
        }
        let (min, median, p95, max, avg) = aggregate_stats(&warm_stats);
        println!(
            "  Warm-up signing -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
            min, median, p95, max, avg
        );
        println!("{}", "─".repeat(80));
    }

    bench_start_nonce = fetch_next_nonce(client, api_key).await?;
    let mut signing_ms = Vec::with_capacity(order_count);
    let mut network_ms = Vec::with_capacity(order_count);
    let mut total_ms = Vec::with_capacity(order_count);
    let mut success_count = 0usize;

    for idx in 0..order_count {
        let per_order_start = Instant::now();
        let (signed, sign_elapsed) = sign_limit_order(
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
                sign_elapsed
            );
        }

        let network_start = Instant::now();
        let success = submit_signed_payload(stream.connection_mut(), &signed).await?;
        let network_elapsed = network_start.elapsed().as_secs_f64() * 1_000.0;
        let total_elapsed = per_order_start.elapsed().as_secs_f64() * 1_000.0;

        if success {
            success_count += 1;
        }

        if log_each_order {
            println!(
                "  Order {:>3}/{:>3}: total {:.2} ms (sign {:.2} ms | net {:.2} ms) {}",
                idx + 1,
                order_count,
                total_elapsed,
                sign_elapsed,
                network_elapsed,
                if success { "✓" } else { "✗" }
            );
        }

        signing_ms.push(sign_elapsed);
        network_ms.push(network_elapsed);
        total_ms.push(total_elapsed);

        if let Some(pause) = pause_duration {
            tokio::time::sleep(pause).await;
        }
    }

    println!("\n{}", "═".repeat(80));
    println!("Summary");
    println!("{}", "═".repeat(80));
    println!("Orders: {}", order_count);
    println!("Success: {}", success_count);

    let (sign_min, sign_median, sign_p95, sign_max, sign_avg) = aggregate_stats(&signing_ms);
    println!(
        "Signing  -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        sign_min, sign_median, sign_p95, sign_max, sign_avg
    );

    let (net_min, net_median, net_p95, net_max, net_avg) = aggregate_stats(&network_ms);
    println!(
        "Network  -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        net_min, net_median, net_p95, net_max, net_avg
    );

    let (total_min, total_median, total_p95, total_max, total_avg) = aggregate_stats(&total_ms);
    println!(
        "Total    -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        total_min, total_median, total_p95, total_max, total_avg
    );

    println!("\n{}", "═".repeat(80));
    println!("⚠ Remember to cancel test orders if necessary.");
    println!("{}", "═".repeat(80));

    Ok(())
}
