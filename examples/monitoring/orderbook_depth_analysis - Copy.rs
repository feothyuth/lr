//! Order Book Depth Analysis using the modern Lighter Rust SDK.
//!
//! Subscribes to the public order book feed, computes liquidity depth metrics,
//! and prints a live dashboard showing best bid/ask, spread, and depth at
//! configurable distances from the mid price.

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::ExampleContext;
use futures_util::StreamExt;
use lighter_client::ws_client::{OrderBookLevel, OrderBookState, WsEvent};
use std::cmp::Ordering;
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

    let mut stream = ctx
        .ws_builder()
        .subscribe_order_book(market)
        .subscribe_market_stats(market)
        .subscribe_bbo(market)
        .subscribe_trade(market)
        .connect()
        .await?;
    let mut update_count = 0u32;
    let mut bbo_quote = BboQuote::default();

    while let Some(event) = stream.next().await {
        match event? {
            WsEvent::Connected => println!("🔗 Connected to Lighter WebSocket"),
            WsEvent::OrderBook(update) => {
                update_count += 1;
                render_depth_dashboard(update.state, update_count, bbo_quote);
            }
            WsEvent::Pong => {
                // keep-alive acknowledgement
            }
            WsEvent::BBO(event) => {
                bbo_quote.update(event.best_bid.as_deref(), event.best_ask.as_deref());
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

fn render_depth_dashboard(state: OrderBookState, update: u32, bbo: BboQuote) {
    let bids = collect_levels(&state.bids);
    let asks = collect_levels(&state.asks);

    // Clear screen and move cursor home
    print!("\x1B[2J\x1B[1;1H");
    println!("{}", "═".repeat(80));
    println!("📊 ORDER BOOK DEPTH ANALYSIS - Update #{}", update);
    println!("{}\n", "═".repeat(80));

    let best_bid = choose_best(&bids, bbo.bid, true);
    let best_ask = choose_best(&asks, bbo.ask, false);

    if let (Some((bid, best_bid_size)), Some((ask, best_ask_size))) = (best_bid, best_ask) {
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
                println!("  {:>12.2}  {:>12.4}  {:>12.4}", price, bid_size, ask_size);
            }
        }
    } else {
        println!("⚠️  Order book is empty");
    }
}

type PriceMap = BTreeMap<PriceKey, f64>;

fn collect_levels(levels: &[OrderBookLevel]) -> PriceMap {
    let mut map = BTreeMap::new();
    for level in levels {
        // Prefer remaining_base_amount (current size) over size (initial size)
        let size_str = if let Some(remaining) = &level.remaining_base_amount {
            remaining.as_str()
        } else {
            level.size.as_str()
        };
        if let (Ok(size), Ok(price)) = (size_str.parse::<f64>(), level.price.parse::<f64>()) {
            // Filter out levels with zero or negative size (filled/cancelled orders)
            if size > 0.0 && price.is_finite() && price > 0.0 {
                if let Some(key) = PriceKey::new(price) {
                    map.insert(key, size);
                }
            }
        }
    }
    map
}

fn choose_best(
    levels: &PriceMap,
    override_price: Option<f64>,
    want_high: bool,
) -> Option<(f64, f64)> {
    let fallback = if want_high {
        levels
            .iter()
            .rev()
            .next()
            .map(|(price, size)| (price.value(), *size))
    } else {
        levels
            .iter()
            .next()
            .map(|(price, size)| (price.value(), *size))
    };

    // Prefer BBO price when available (more reliable than potentially stale order book)
    match (override_price, fallback) {
        (Some(bbo_price), Some((book_price, book_size)))
            if approx_equal(book_price, bbo_price) =>
        {
            // BBO matches book - try to get size from book at BBO price
            PriceKey::new(bbo_price)
                .and_then(|key| levels.get(&key).copied())
                .map(|size| (bbo_price, size))
                .or(Some((book_price, book_size)))
        }
        (Some(bbo_price), _) => {
            // BBO available but doesn't match book - trust BBO price
            // Try to find size in book, otherwise use 0
            PriceKey::new(bbo_price)
                .and_then(|key| levels.get(&key).copied())
                .map(|size| (bbo_price, size))
                .or(Some((bbo_price, 0.0)))
        }
        (None, Some(best)) => Some(best),  // No BBO - use book
        _ => None,
    }
}

fn depth_at_percentage(levels: &PriceMap, mid: f64, pct: f64, is_bid: bool) -> f64 {
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
        let price = price.value();
        let include = if is_bid {
            price >= limit
        } else {
            price <= limit
        };
        if include {
            total += size;
        }
    }
    total
}

fn top_levels(bids: &PriceMap, asks: &PriceMap, limit: usize) -> Vec<(f64, f64, f64)> {
    let mut entries = Vec::new();

    for (price, bid_size) in bids.iter().rev().take(limit) {
        let ask_size = asks.get(price).copied().unwrap_or(0.0);
        entries.push((price.value(), *bid_size, ask_size));
    }

    if entries.len() < limit {
        for (price, ask_size) in asks.iter().take(limit - entries.len()) {
            entries.push((price.value(), 0.0, *ask_size));
        }
    }

    entries
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PriceKey(f64);

impl PriceKey {
    fn new(value: f64) -> Option<Self> {
        if value.is_finite() {
            Some(Self(value))
        } else {
            None
        }
    }

    fn value(self) -> f64 {
        self.0
    }
}

impl Eq for PriceKey {}

impl PartialOrd for PriceKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

impl Ord for PriceKey {
    fn cmp(&self, other: &Self) -> Ordering {
        // Safe unwrap: only finite values are inserted via PriceKey::new
        self.partial_cmp(other).unwrap()
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct BboQuote {
    bid: Option<f64>,
    ask: Option<f64>,
}

impl BboQuote {
    fn update(&mut self, bid: Option<&str>, ask: Option<&str>) {
        self.bid = parse_price(bid);
        self.ask = parse_price(ask);
    }
}

fn parse_price(value: Option<&str>) -> Option<f64> {
    value
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

fn approx_equal(lhs: f64, rhs: f64) -> bool {
    let tolerance = (lhs.abs().max(rhs.abs()) * 1e-6).max(0.01);
    (lhs - rhs).abs() <= tolerance
}
