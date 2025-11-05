// Benchmark: WebSocket vs REST API
// Tests sustained throughput with multiple sequential transactions
// This shows where WebSocket really shines!

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::ExampleContext;
use futures_util::StreamExt;
use lighter_client::ws_client::WsEvent;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("bench_ws_vs_rest")).await?;
    let client = ctx.client();
    let market = ctx.market_id();

    println!("\n{}", "═".repeat(80));
    println!("⚡ WebSocket vs REST API Latency Benchmark");
    println!("Testing: 5 sequential orderbook fetches (read-only, safe)");
    println!("{}\n", "═".repeat(80));

    // Test parameters
    let num_requests = 5;

    println!("📊 Test Setup:");
    println!("   Requests:      {}", num_requests);
    println!("   Account:       {}", ctx.account_id().into_inner());
    println!("   API Key:       {}", ctx.api_key_index().into_inner());
    println!();

    // ========== REST API Benchmark ==========
    println!("🔴 REST API Benchmark");
    println!("{}", "─".repeat(80));

    let rest_start = Instant::now();
    let mut rest_times = Vec::new();

    for i in 0..num_requests {
        let req_start = Instant::now();

        let _response = client.orders().book(market, 5).await?;

        let req_duration = req_start.elapsed();
        rest_times.push(req_duration.as_millis());
        println!("   Request {}: {}ms", i + 1, req_duration.as_millis());
    }

    let rest_total = rest_start.elapsed();
    let rest_avg = rest_times.iter().sum::<u128>() / rest_times.len() as u128;

    println!();
    println!("   Total Time:    {}ms", rest_total.as_millis());
    println!("   Average:       {}ms per request", rest_avg);
    println!("   Throughput:    {:.1} req/sec", 1000.0 / rest_avg as f64);
    println!();

    // ========== WebSocket Benchmark ==========
    println!("🟢 WebSocket Benchmark");
    println!("{}", "─".repeat(80));

    // Connect once
    let connect_start = Instant::now();
    let builder = client.ws().subscribe_order_book(market);
    let mut stream = builder.connect().await?;
    let connect_duration = connect_start.elapsed();
    println!(
        "   Connect:       {}ms (one-time overhead)",
        connect_duration.as_millis()
    );

    let ws_start = Instant::now();
    let mut ws_times = Vec::new();

    // Measure orderbook updates received via WebSocket
    for i in 0..num_requests {
        let req_start = Instant::now();

        // Send a ping and wait for pong to measure round-trip time
        stream.connection_mut().ping().await?;

        // Wait for next event (should be a Pong)
        while let Some(event_result) = stream.next().await {
            match event_result {
                Ok(WsEvent::Pong) => {
                    let req_duration = req_start.elapsed();
                    ws_times.push(req_duration.as_millis());
                    println!("   Request {}: {}ms", i + 1, req_duration.as_millis());
                    break;
                }
                Ok(WsEvent::OrderBook(_)) => {
                    // Ignore orderbook updates while waiting for pong
                    continue;
                }
                Ok(_) => continue,
                Err(e) => {
                    println!("   Request {} failed: {}", i + 1, e);
                    break;
                }
            }
        }

        // Small delay to avoid rate limiting
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    let ws_total = ws_start.elapsed();
    let ws_avg = if !ws_times.is_empty() {
        ws_times.iter().sum::<u128>() / ws_times.len() as u128
    } else {
        0
    };

    println!();
    println!(
        "   Total Time:    {}ms (excluding connect)",
        ws_total.as_millis()
    );
    println!("   Average:       {}ms per request", ws_avg);
    if ws_avg > 0 {
        println!("   Throughput:    {:.1} req/sec", 1000.0 / ws_avg as f64);
    }
    println!();

    // ========== Comparison ==========
    println!("{}", "═".repeat(80));
    println!("📊 Comparison Summary");
    println!("{}", "═".repeat(80));
    println!();

    println!("REST API:");
    println!("   Total:         {}ms", rest_total.as_millis());
    println!("   Per Request:   {}ms", rest_avg);
    println!();

    println!("WebSocket:");
    println!(
        "   Connect:       {}ms (one-time)",
        connect_duration.as_millis()
    );
    println!(
        "   Total:         {}ms (excluding connect)",
        ws_total.as_millis()
    );
    println!("   Per Request:   {}ms", ws_avg);
    println!();

    if ws_avg > 0 {
        let improvement = if rest_avg > ws_avg {
            let pct = ((rest_avg - ws_avg) as f64 / rest_avg as f64) * 100.0;
            format!("WebSocket is {:.1}% faster per request", pct)
        } else {
            let pct = ((ws_avg - rest_avg) as f64 / ws_avg as f64) * 100.0;
            format!("REST is {:.1}% faster per request", pct)
        };

        println!("⚡ {}", improvement);
        println!();

        // Calculate break-even point
        if rest_avg > ws_avg {
            let breakeven = connect_duration.as_millis() / (rest_avg.saturating_sub(ws_avg));
            println!("💡 WebSocket pays off after ~{} requests", breakeven);
            println!("   (when connection overhead is amortized)");
        }
    }

    println!();
    println!("{}", "═".repeat(80));
    println!("✅ Benchmark complete!");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
