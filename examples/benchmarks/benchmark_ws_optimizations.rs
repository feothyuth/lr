//! Benchmark WebSocket optimizations
//!
//! Measures performance improvements from:
//! 1. BBO caching (only emit when changed)
//! 2. Fast-path zero checking (avoid float parsing)
//!
//! Usage:
//!   cargo run --example benchmark_ws_optimizations --release

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::ExampleContext;
use futures_util::StreamExt;
use lighter_client::ws_client::WsEvent;
use std::time::{Duration, Instant};

struct BenchmarkStats {
    total_events: u64,
    orderbook_updates: u64,
    bbo_events: u64,
    bbo_changes: u64,
    position_events: u64,
    trade_events: u64,
    elapsed: Duration,
}

impl BenchmarkStats {
    fn new() -> Self {
        Self {
            total_events: 0,
            orderbook_updates: 0,
            bbo_events: 0,
            bbo_changes: 0,
            position_events: 0,
            trade_events: 0,
            elapsed: Duration::ZERO,
        }
    }

    fn print_report(&self) {
        let seconds = self.elapsed.as_secs_f64();

        println!("\n{}", "═".repeat(80));
        println!("📊 BENCHMARK RESULTS");
        println!("{}", "═".repeat(80));

        println!("\n🔧 Configuration:");
        println!("   JSON Parser: serde_json");
        println!("   BBO Caching: ENABLED (only emit on change)");
        println!("   Zero Check:  Fast-path string comparison");

        println!("\n⏱️  Duration: {:.2}s", seconds);

        println!("\n📈 Event Throughput:");
        println!("   Total Events:      {:>8} ({:>8.1} events/sec)",
            self.total_events, self.total_events as f64 / seconds);
        println!("   OrderBook Updates: {:>8} ({:>8.1} updates/sec)",
            self.orderbook_updates, self.orderbook_updates as f64 / seconds);
        println!("   BBO Events:        {:>8} ({:>8.1} events/sec)",
            self.bbo_events, self.bbo_events as f64 / seconds);
        println!("   Position Events:   {:>8}", self.position_events);
        println!("   Trade Events:      {:>8}", self.trade_events);

        println!("\n🎯 BBO Efficiency:");
        if self.orderbook_updates > 0 {
            let bbo_reduction = 100.0 * (1.0 - (self.bbo_events as f64 / self.orderbook_updates as f64));
            println!("   OrderBook → BBO:   {:>8} → {:>8} ({:.1}% reduction)",
                self.orderbook_updates, self.bbo_events, bbo_reduction);
        }
        if self.bbo_events > 0 {
            println!("   BBO Changes:       {:>8} / {:>8} ({:.1}% actually changed)",
                self.bbo_changes, self.bbo_events,
                100.0 * self.bbo_changes as f64 / self.bbo_events as f64);
        }

        println!("\n⚡ Performance Metrics:");
        if seconds > 0.0 {
            let avg_event_latency_us = (seconds * 1_000_000.0) / self.total_events as f64;
            println!("   Avg Event Latency: {:.2}µs", avg_event_latency_us);
        }

        println!("\n{}", "═".repeat(80));
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("benchmark_ws_optimizations")).await?;
    let market = ctx.market_id();
    let account = ctx.account_id();

    println!("\n{}", "═".repeat(80));
    println!("🚀 WebSocket Performance Benchmark");
    println!("{}", "═".repeat(80));
    println!("Market:  {}", market.into_inner());
    println!("Account: {}", account.into_inner());
    println!("\nCollecting data for 30 seconds...");
    println!("{}", "═".repeat(80));
    println!();

    // Create stream with all channels
    let mut stream = ctx
        .ws_builder()
        .subscribe_order_book(market)
        .subscribe_bbo(market)
        .subscribe_account_market_positions(market, account)
        .subscribe_account_market_trades(market, account)
        .connect()
        .await?;

    let mut stats = BenchmarkStats::new();
    let start = Instant::now();
    let benchmark_duration = Duration::from_secs(30);

    let mut last_bid: Option<String> = None;
    let mut last_ask: Option<String> = None;

    while start.elapsed() < benchmark_duration {
        match tokio::time::timeout(Duration::from_secs(1), stream.next()).await {
            Ok(Some(Ok(event))) => {
                stats.total_events += 1;

                match event {
                    WsEvent::Connected => {
                        println!("🔗 Connected to WebSocket");
                    }
                    WsEvent::OrderBook(_) => {
                        stats.orderbook_updates += 1;
                    }
                    WsEvent::BBO(bbo) => {
                        stats.bbo_events += 1;

                        // Check if BBO actually changed (verify cache is working)
                        let changed = last_bid.as_ref() != bbo.best_bid.as_ref()
                                   || last_ask.as_ref() != bbo.best_ask.as_ref();

                        if changed {
                            stats.bbo_changes += 1;
                            last_bid = bbo.best_bid;
                            last_ask = bbo.best_ask;
                        }
                    }
                    WsEvent::Account(envelope) => {
                        let event_data = envelope.event.as_value();
                        let channel = event_data
                            .get("channel")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        if channel.contains("account_positions") {
                            stats.position_events += 1;
                        } else if channel.contains("account_trades") {
                            stats.trade_events += 1;
                        }
                    }
                    WsEvent::Pong => {
                        // Keep-alive
                    }
                    _ => {
                        // Other events
                    }
                }

                // Progress indicator
                if stats.total_events % 100 == 0 {
                    let elapsed = start.elapsed().as_secs_f64();
                    print!("\r⏳ {} events processed ({:.1}s elapsed)...",
                        stats.total_events, elapsed);
                    std::io::Write::flush(&mut std::io::stdout()).ok();
                }
            }
            Ok(Some(Err(e))) => {
                eprintln!("\n❌ Error: {}", e);
                break;
            }
            Ok(None) => {
                println!("\n✅ Stream ended");
                break;
            }
            Err(_) => {
                // Timeout, continue
            }
        }
    }

    stats.elapsed = start.elapsed();
    println!("\r{}", " ".repeat(60)); // Clear progress line
    stats.print_report();

    Ok(())
}
