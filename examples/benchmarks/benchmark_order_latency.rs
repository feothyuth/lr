// Benchmark Order Placement Latency via WebSocket
// Measures signing time, network time, and total end-to-end latency
// Useful for optimizing grid bots and market makers

#[path = "../common/example_context.rs"]
mod common;

use anyhow::{Context, Result};
use common::{connect_private_stream, submit_signed_payload, ExampleContext};
use lighter_client::types::{BaseQty, Expiry, Price};
use std::time::Instant;

struct LatencyMeasurement {
    signing_ms: f64,
    network_ms: f64,
    total_ms: f64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("benchmark_order_latency")).await?;
    let market = ctx.market_id();
    let client = ctx.client();
    let signer = ctx.signer()?;

    println!("\n{}", "═".repeat(80));
    println!("⚡ Order Placement Latency Benchmark");
    println!("{}\n", "═".repeat(80));

    // Fetch current market price
    print!("📈 Fetching current market price... ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let order_book = client.orders().book(market, 5).await?;

    let best_bid: f64 = order_book
        .bids
        .first()
        .context("No bids available")?
        .price
        .parse()?;
    println!("✓ (${:.2})", best_bid);

    // Calculate safe price (20% below market)
    let safe_price = best_bid * 0.80;
    let test_size = 0.005; // Minimum size

    println!("\n{}", "═".repeat(80));
    println!("Test Configuration");
    println!("{}", "═".repeat(80));
    println!("Market: ETH-PERP (ID: {})", market.into_inner());
    println!("Side: BUY");
    println!("Price: ${:.2} (20% below bid)", safe_price);
    println!("Size: {} ETH", test_size);
    println!("Type: LIMIT (GoodTillTime)");
    println!("Note: Safe price, won't fill immediately");

    // Connect to WebSocket
    print!("\n🌐 Connecting to WebSocket... ");
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let builder = client
        .ws()
        .subscribe_transactions()
        .subscribe_executed_transactions();
    let (mut stream, _auth_token) = connect_private_stream(&ctx, builder).await?;
    println!("✓");

    println!("\n{}", "═".repeat(80));
    println!("Running Benchmark (1 order)");
    println!("{}\n", "═".repeat(80));

    // Run benchmark
    let mut measurements = Vec::new();
    let num_orders = 1;

    for i in 0..num_orders {
        let measurement = place_order_with_timing(
            &mut stream,
            signer,
            market,
            safe_price,
            test_size,
            i + 1,
            num_orders,
        )
        .await?;

        measurements.push(measurement);

        // Delay between orders
        if i < num_orders - 1 {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
    }

    // Calculate statistics
    println!("\n{}", "═".repeat(80));
    println!("Results Summary");
    println!("{}", "═".repeat(80));

    println!("\nOrders placed: {}", measurements.len());
    let success_count = measurements.len();
    let error_count = num_orders - success_count;
    println!("Success: {} ✓", success_count);
    println!("Errors: {} ✗", error_count);

    if !measurements.is_empty() {
        let mut total_times: Vec<f64> = measurements.iter().map(|m| m.total_ms).collect();
        total_times.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let min = total_times.first().unwrap();
        let max = total_times.last().unwrap();
        let avg = total_times.iter().sum::<f64>() / total_times.len() as f64;
        let p50 = total_times[total_times.len() / 2];
        let p99 = total_times[(total_times.len() as f64 * 0.99) as usize];

        println!("\nLatency Statistics (end-to-end):");
        println!("  Min:  {:.2}ms", min);
        println!("  P50:  {:.2}ms", p50);
        println!("  Avg:  {:.2}ms", avg);
        println!("  P99:  {:.2}ms", p99);
        println!("  Max:  {:.2}ms", max);

        // Analysis
        println!("\nAnalysis:");
        if avg < 50.0 {
            println!("  ✓ GOOD: <50ms (competitive for grid bots)");
        } else if avg < 100.0 {
            println!("  ⚠ OKAY: 50-100ms (acceptable for most strategies)");
        } else {
            println!("  ✗ SLOW: >100ms (may need optimization)");
        }

        // Calculate average component breakdown
        let avg_signing =
            measurements.iter().map(|m| m.signing_ms).sum::<f64>() / measurements.len() as f64;
        let avg_network =
            measurements.iter().map(|m| m.network_ms).sum::<f64>() / measurements.len() as f64;

        // Project Tokyo latency
        let current_network_rtt = avg_network;
        let tokyo_network_rtt = 1.5; // Estimated Tokyo RTT (1-2ms)
        let _tokyo_total = avg_signing + tokyo_network_rtt;

        println!("\nTokyo Projection:");
        println!("  Current network RTT: ~{:.1}ms", current_network_rtt);
        println!("  Tokyo network RTT: ~1-2ms (estimated)");
        println!(
            "  Tokyo total latency: ~{:.1}-{:.1}ms",
            avg_signing + 1.0,
            avg_signing + 2.0
        );
    }

    println!("\n{}", "═".repeat(80));
    println!("✅ Benchmark complete!");
    println!("{}\n", "═".repeat(80));

    Ok(())
}

async fn place_order_with_timing(
    stream: &mut lighter_client::ws_client::WsStream,
    signer: &lighter_client::signer_client::SignerClient,
    market: lighter_client::types::MarketId,
    price: f64,
    size: f64,
    order_num: usize,
    total_orders: usize,
) -> Result<LatencyMeasurement> {
    // Start total timer
    let total_start = Instant::now();

    // Convert to integer format
    let price_int = (price * 100.0).round() as i64;
    let size_int = (size * 10000.0).round() as i64;

    // Generate unique client order index
    let client_order_index = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;

    // Calculate expiry (1 hour)
    let expiry = Expiry::from_now(time::Duration::hours(1));

    let price_ticks = Price::ticks(price_int);
    let base_qty =
        BaseQty::try_from(size_int).map_err(|e| anyhow::anyhow!("Invalid quantity: {}", e))?;

    // Measure signing time
    let signing_start = Instant::now();
    let signed = signer
        .sign_create_order(
            market.into(),
            client_order_index,
            base_qty.into_inner(),
            price_ticks.into_ticks() as i32,
            false, // BUY
            signer.order_type_limit(),
            signer.order_time_in_force_good_till_time(),
            false,
            0,
            expiry.into_unix(),
            None,
            None,
        )
        .await?;
    let signing_ms = signing_start.elapsed().as_micros() as f64 / 1000.0;

    // Measure network time
    let network_start = Instant::now();
    let success = submit_signed_payload(stream.connection_mut(), &signed).await?;

    let network_ms = network_start.elapsed().as_micros() as f64 / 1000.0;
    let total_ms = total_start.elapsed().as_micros() as f64 / 1000.0;

    println!(
        "Order {}/{}: BUY  {} ETH @ ${:.2}",
        order_num, total_orders, size, price
    );

    if success {
        println!("  ✓ SUCCESS in {:.0}ms", total_ms);
        println!("    - Signing: {:.2}ms", signing_ms);
        println!("    - Network: {:.2}ms", network_ms);

        Ok(LatencyMeasurement {
            signing_ms,
            network_ms,
            total_ms,
        })
    } else {
        println!("  ✗ REJECTED in {:.0}ms", total_ms);
        Err(anyhow::anyhow!("Order rejected by exchange"))
    }
}
