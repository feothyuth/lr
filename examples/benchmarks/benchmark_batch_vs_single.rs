// Benchmark batch submission vs single-order submission latency.
//
// Scenario 1:
//   * Sign 50 limit orders (BTC-PERP) and submit them as a single WebSocket batch.
//   * Report signing aggregate, network send time, total elapsed, and success count.
//
// Scenario 2:
//   * Sign and submit the same number of limit orders one-by-one.
//   * Report per-order signing/network/total stats and aggregate summary.
//
// Environment requirements:
//   - LIGHTER_PRIVATE_KEY, LIGHTER_ACCOUNT_INDEX, LIGHTER_API_KEY_INDEX
//   - Optional: LIGHTER_MARKET_ID (defaults via config), LIGHTER_API_URL, LIGHTER_WS_URL
//   - Optional tuning:
//       LATENCY_BATCH_COUNT  (default: 50)
//       LATENCY_SINGLE_COUNT (default: 50)

#[path = "../common/example_context.rs"]
mod common;

use anyhow::{Context, Result};
use common::{connect_private_stream, submit_signed_payload, ExampleContext};
use lighter_client::lighter_client::LighterClient;
use lighter_client::signer_client::SignerClient;
use lighter_client::tx_executor::send_batch_tx_ws;
use lighter_client::types::{ApiKeyIndex, BaseQty, Expiry, MarketId, Nonce, Price};
use std::env;
use std::time::{Duration as StdDuration, Instant};
use time::Duration;

const DEFAULT_BATCH_COUNT: usize = 10;
const DEFAULT_SINGLE_COUNT: usize = 10;
const DEFAULT_SIZE_BASE: f64 = 0.0002;

struct BatchMetrics {
    orders: usize,
    signing_times_ms: Vec<f64>,
    network_ms: f64,
    total_ms: f64,
    success: usize,
}

impl BatchMetrics {
    fn summary(&self) -> (f64, f64, f64, f64) {
        let signing_total = self.signing_times_ms.iter().sum::<f64>();
        let signing_avg = signing_total / self.signing_times_ms.len() as f64;
        let signing_max = self
            .signing_times_ms
            .iter()
            .copied()
            .fold(0.0_f64, f64::max);
        let signing_min = self
            .signing_times_ms
            .iter()
            .copied()
            .fold(f64::MAX, f64::min);
        (signing_total, signing_avg, signing_min, signing_max)
    }
}

struct SingleMetrics {
    orders: usize,
    signing_ms: Vec<f64>,
    network_ms: Vec<f64>,
    total_ms: Vec<f64>,
    success: usize,
}

impl SingleMetrics {
    fn aggregate(stats: &[f64]) -> (f64, f64, f64, f64, f64) {
        let mut values = stats.to_vec();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = *values.first().unwrap_or(&0.0);
        let max = *values.last().unwrap_or(&0.0);
        let sum: f64 = stats.iter().sum();
        let avg = if stats.is_empty() {
            0.0
        } else {
            sum / stats.len() as f64
        };
        let median = if values.is_empty() {
            0.0
        } else {
            values[values.len() / 2]
        };
        let p95_index = ((values.len() as f64) * 0.95).floor() as usize;
        let p95 = values
            .get(p95_index.min(values.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0.0);
        (min, median, p95, max, avg)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lighter_client=info".into()),
        )
        .try_init();

    let batch_count = read_env_usize("LATENCY_BATCH_COUNT", DEFAULT_BATCH_COUNT);
    let single_count = read_env_usize("LATENCY_SINGLE_COUNT", DEFAULT_SINGLE_COUNT);

    let ctx = ExampleContext::initialise(Some("benchmark_batch_vs_single")).await?;
    let market = ctx.market_id();
    let client = ctx.client();
    let signer = ctx.signer()?;
    println!("\n{}", "═".repeat(80));
    println!("🔬 Batch vs. Single Order Latency Benchmark");
    println!("{}\n", "═".repeat(80));

    // Retrieve market metadata for tick/size conversions.
    let metadata = client
        .orders()
        .book_details(Some(market))
        .await
        .context("failed to load market metadata")?;
    let detail = metadata
        .order_book_details
        .into_iter()
        .find(|d| d.market_id == market.into_inner())
        .context("market metadata missing for requested market")?;

    let price_multiplier: i64 = 10_i64
        .checked_pow(detail.price_decimals as u32)
        .context("price_decimals overflow")?;
    let size_multiplier: i64 = 10_i64
        .checked_pow(detail.size_decimals as u32)
        .context("size_decimals overflow")?;

    let price_override = read_env_f64_optional("LATENCY_PRICE_USD");
    let mut size_base = read_env_f64("LATENCY_SIZE_BASE", DEFAULT_SIZE_BASE);

    let price_usd = match price_override {
        Some(value) => value,
        None => {
            let ob = client.orders().book(market, 5).await?;
            let best_bid = ob
                .bids
                .first()
                .and_then(|bid| bid.price.parse::<f64>().ok())
                .context("no bids available to derive price")?;
            best_bid * 0.9 // place 10% below to avoid fills
        }
    };

    let price_ticks_raw = (price_usd * price_multiplier as f64).round() as i64;

    let _price_ticks = Price::ticks(price_ticks_raw);
    let min_base = detail
        .min_base_amount
        .parse::<f64>()
        .unwrap_or(DEFAULT_SIZE_BASE);
    let min_quote = detail.min_quote_amount.parse::<f64>().unwrap_or(0.0);

    if size_base < min_base {
        println!(
            "Requested size {:.6} below min_base {:.6}; using minimum.",
            size_base, min_base
        );
        size_base = min_base;
    }

    if price_usd * size_base < min_quote {
        let adjusted = (min_quote / price_usd).max(size_base);
        println!(
            "Order value {:.6} below min_quote {:.6}; adjusting size to {:.6}.",
            price_usd * size_base,
            min_quote,
            adjusted
        );
        size_base = adjusted;
    }

    let size_raw = (size_base * size_multiplier as f64).round() as i64;

    let base_qty = BaseQty::try_from(size_raw)
        .map_err(|err| anyhow::anyhow!("size too small for market decimals: {err}"))?;

    println!("Market ID: {}", market.into_inner());
    println!("Price: ${:.2}", price_usd);
    println!("Size: {} base", size_base);
    println!("Batch orders: {}", batch_count);
    println!("Single orders: {}", single_count);

    // Prepare WebSocket connection for submissions.
    let builder = client
        .ws()
        .subscribe_transactions()
        .subscribe_executed_transactions();
    let (mut stream, _auth_token) = connect_private_stream(&ctx, builder).await?;

    println!("\n{}", "═".repeat(80));
    println!("Scenario 1: Batch submission ({} orders)", batch_count);
    println!("{}", "═".repeat(80));
    let batch_metrics = benchmark_batch(
        &mut stream,
        client,
        signer,
        market,
        price_ticks_raw as i32,
        base_qty,
        batch_count,
    )
    .await?;

    // Small pause to avoid mixing results.
    tokio::time::sleep(StdDuration::from_millis(400)).await;

    // Reconnect to ensure clean stream for single submission test.
    drop(stream);
    let (mut stream, _auth_token) = connect_private_stream(
        &ctx,
        client
            .ws()
            .subscribe_transactions()
            .subscribe_executed_transactions(),
    )
    .await?;

    println!("\n{}", "═".repeat(80));
    println!("Scenario 2: Single submissions ({} orders)", single_count);
    println!("{}", "═".repeat(80));
    let single_metrics = benchmark_single(
        &mut stream,
        client,
        signer,
        market,
        price_ticks_raw as i32,
        base_qty,
        single_count,
    )
    .await?;

    println!("\n{}", "═".repeat(80));
    println!("Summary");
    println!("{}", "═".repeat(80));

    let (sign_total, sign_avg, sign_min, sign_max) = batch_metrics.summary();
    println!("Batch:");
    println!("  Orders: {}", batch_metrics.orders);
    println!("  Success: {}", batch_metrics.success);
    println!(
        "  Signing total: {:.2} ms (avg {:.2} ms | min {:.2} ms | max {:.2} ms)",
        sign_total, sign_avg, sign_min, sign_max
    );
    println!("  Network send: {:.2} ms", batch_metrics.network_ms);
    println!("  Total elapsed: {:.2} ms", batch_metrics.total_ms);

    let (single_sign_min, single_sign_median, single_sign_p95, single_sign_max, single_sign_avg) =
        SingleMetrics::aggregate(&single_metrics.signing_ms);
    let (single_net_min, single_net_median, single_net_p95, single_net_max, single_net_avg) =
        SingleMetrics::aggregate(&single_metrics.network_ms);
    let (
        single_total_min,
        single_total_median,
        single_total_p95,
        single_total_max,
        single_total_avg,
    ) = SingleMetrics::aggregate(&single_metrics.total_ms);

    println!("\nSingle submissions:");
    println!("  Orders: {}", single_metrics.orders);
    println!("  Success: {}", single_metrics.success);
    println!(
        "  Signing  -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        single_sign_min, single_sign_median, single_sign_p95, single_sign_max, single_sign_avg
    );
    println!(
        "  Network  -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        single_net_min, single_net_median, single_net_p95, single_net_max, single_net_avg
    );
    println!(
        "  Total    -> min {:.2} ms | median {:.2} ms | p95 {:.2} ms | max {:.2} ms | avg {:.2} ms",
        single_total_min, single_total_median, single_total_p95, single_total_max, single_total_avg
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
    base_qty: BaseQty,
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

async fn benchmark_single(
    stream: &mut lighter_client::ws_client::WsStream,
    client: &LighterClient,
    signer: &SignerClient,
    market: MarketId,
    price_ticks: i32,
    base_qty: BaseQty,
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

fn read_env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn read_env_f64(key: &str, default: f64) -> f64 {
    read_env_f64_optional(key).unwrap_or(default)
}

fn read_env_f64_optional(key: &str) -> Option<f64> {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| *value > 0.0)
}
