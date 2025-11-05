//! Order Book Depth Analysis using the modern Lighter Rust SDK.
//!
//! Subscribes to the public order book feed, computes liquidity depth metrics,
//! and prints a live dashboard showing best bid/ask, spread, and depth at
//! configurable distances from the mid price.

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::{connect_public_stream, ExampleContext};
use futures_util::StreamExt;
use lighter_client::ws_client::{OrderBookLevel, OrderBookState, WsEvent};
use std::collections::BTreeMap;

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("orderbook_depth_analysis")).await?;
    let market = ctx.market_id();

    println!("\n{}", "═".repeat(80));
    println!(
        "📊 Order Book Depth Analysis (market {})",
        market.into_inner()
    );
    println!("{}\n", "═".repeat(80));

    let mut stream = connect_public_stream(&ctx, market).await?;
    let mut update_count = 0u32;

    while let Some(event) = stream.next().await {
        match event? {
            WsEvent::Connected => println!("🔗 Connected to Lighter WebSocket"),
            WsEvent::OrderBook(update) => {
                update_count += 1;
                render_depth_dashboard(update.state, update_count);
            }
            WsEvent::Pong => {
                // keep-alive acknowledgement
            }
            WsEvent::Closed(frame) => {
                println!("🔌 Connection closed: {:?}", frame);
                break;
            }
            other => {
                // Ignore other public channel chatter (trades, stats, etc.)
                common::display_ws_event(other);
            }
        }
    }

    println!("\n✅ Stream finished");
    Ok(())
}

fn render_depth_dashboard(state: OrderBookState, update: u32) {
    let bids = collect_levels(&state.bids);
    let asks = collect_levels(&state.asks);

    // Clear screen and move cursor home
    print!("\x1B[2J\x1B[1;1H");
    println!("{}", "═".repeat(80));
    println!("📊 ORDER BOOK DEPTH ANALYSIS - Update #{}", update);
    println!("{}\n", "═".repeat(80));

    if let (Some((best_bid_price, best_bid_size)), Some((best_ask_price, best_ask_size))) =
        (bids.iter().rev().next(), asks.iter().next())
    {
        let bid = best_bid_price.parse::<f64>().unwrap_or_default();
        let ask = best_ask_price.parse::<f64>().unwrap_or_default();

        if bid > 0.0 && ask > 0.0 {
            let spread = ask - bid;
            let mid = (bid + ask) / 2.0;
            let spread_bps = if bid > 0.0 {
                (spread / bid) * 10_000.0
            } else {
                0.0
            };

            println!("📍 TOP OF BOOK");
            println!("{}", "─".repeat(80));
            println!("  Best Bid:  ${:>10.2}  x  {:>8.4}", bid, best_bid_size);
            println!("  Best Ask:  ${:>10.2}  x  {:>8.4}", ask, best_ask_size);
            println!("  Mid Price: ${:>10.2}", mid);
            println!("  Spread:    ${:>10.2}  ({:.2} bps)", spread, spread_bps);

            println!("\n📏 LIQUIDITY DEPTH (cumulative size)");
            println!("{}", "─".repeat(80));
            for pct in [0.1, 0.5, 1.0, 2.0, 5.0] {
                let bid_depth = depth_at_percentage(&bids, mid, pct, true);
                let ask_depth = depth_at_percentage(&asks, mid, pct, false);
                println!(
                    "  {:>4.1}% from mid → Bid depth: {:>10.4}  |  Ask depth: {:>10.4}",
                    pct, bid_depth, ask_depth
                );
            }

            println!("\n📈 LIQUIDITY HEATMAP (Top 10 levels)");
            println!("{}", "─".repeat(80));
            println!("  {:>12}  {:>12}  {:>12}", "Price", "Bid Size", "Ask Size");
            println!("  {:>12}  {:>12}  {:>12}", "──────", "───────", "──────");
            for (price, bid_size, ask_size) in top_levels(&bids, &asks, 10) {
                println!("  {:>12}  {:>12.4}  {:>12.4}", price, bid_size, ask_size);
            }
        }
    } else {
        println!("⚠️  Order book is empty");
    }
}

fn collect_levels(levels: &[OrderBookLevel]) -> BTreeMap<String, f64> {
    let mut map = BTreeMap::new();
    for level in levels {
        let size_str = if let Some(remaining) = &level.remaining_base_amount {
            remaining.as_str()
        } else {
            level.size.as_str()
        };
        if let Ok(size) = size_str.parse::<f64>() {
            if size > 0.0 {
                map.insert(level.price.clone(), size);
            }
        }
    }

    map
}

fn depth_at_percentage(levels: &BTreeMap<String, f64>, mid: f64, pct: f64, is_bid: bool) -> f64 {
    if mid == 0.0 {
        return 0.0;
    }

    let limit = if is_bid {
        mid * (1.0 - pct / 100.0)
    } else {
        mid * (1.0 + pct / 100.0)
    };

    let mut total = 0.0;
    for (price, size) in levels {
        if let Ok(price_f) = price.parse::<f64>() {
            let include = if is_bid {
                price_f >= limit
            } else {
                price_f <= limit
            };
            if include {
                total += size;
            }
        }
    }
    total
}

fn top_levels(
    bids: &BTreeMap<String, f64>,
    asks: &BTreeMap<String, f64>,
    limit: usize,
) -> Vec<(String, f64, f64)> {
    let mut entries = Vec::new();

    for (price, bid_size) in bids.iter().rev().take(limit) {
        let ask_size = asks.get(price).copied().unwrap_or(0.0);
        entries.push((price.clone(), *bid_size, ask_size));
    }

    if entries.len() < limit {
        for (price, ask_size) in asks.iter().take(limit - entries.len()) {
            entries.push((price.clone(), 0.0, *ask_size));
        }
    }

    entries
}
