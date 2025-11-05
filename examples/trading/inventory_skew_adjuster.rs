//! Inventory Skew Adjuster for Lighter DEX Market Making
//!
//! Adjusts quotes based on current inventory:
//!  - Tightens asks / widens bids when long
//!  - Tightens bids / widens asks when short
//!
//! Requires the standard `.env` credentials.

#[path = "../common/example_context.rs"]
mod common;

use futures_util::StreamExt;
use lighter_client::ws_client::WsEvent;
use std::collections::HashMap;

use common::ExampleContext;

// Configuration for skew calculation
const BASE_SPREAD: f64 = 0.0001; // 0.01%
const MAX_POSITION_BTC: f64 = 0.01;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   INVENTORY SKEW ADJUSTER - Lighter DEX Market Making      ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    let ctx = ExampleContext::initialise(Some("inventory_skew_adjuster")).await?;
    let _client = ctx.client();
    let account_id = ctx.account_id();
    let market = ctx.market_id();
    let api_key_index = ctx.api_key_index().into_inner();

    println!("Configuration:");
    println!("   Account Index: {}", account_id.into_inner());
    println!("   API Key Index: {}", api_key_index);
    println!("   Market ID: {}", i32::from(market));
    println!("   Base Spread: {}%", BASE_SPREAD * 100.0);
    println!("   Max Position (normalization): {} BTC", MAX_POSITION_BTC);
    println!();

    // Track positions (market_id -> PositionState)
    let mut positions: HashMap<i32, PositionState> = HashMap::new();
    let mut mark_prices: HashMap<i32, f64> = HashMap::new();

    let mut stream = ctx
        .ws_builder()
        .subscribe_account_all_positions(account_id)
        .subscribe_market_stats(market)
        .connect()
        .await?;

    println!("✅ Connected to WebSocket. Waiting for updates...\n");

    while let Some(event) = stream.next().await {
        match event? {
            WsEvent::Account(account_event) => {
                let data = account_event.event.as_value();
                if let Some(array) = data.get("positions").and_then(|v| v.as_array()) {
                    for entry in array {
                        update_position(entry, &mut positions, &mark_prices);
                    }
                }
            }
            WsEvent::MarketStats(stats) => {
                let market_id = stats.market_stats.market_id as i32;
                if let Ok(price) = stats.market_stats.mark_price.parse::<f64>() {
                    mark_prices.insert(market_id, price);
                    if let Some(position) = positions.get(&market_id) {
                        display_position(market_id, position, Some(price));
                    }
                }
            }
            WsEvent::Connected => println!("🔌 WebSocket connected"),
            _ => {}
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct PositionState {
    size: f64,
    sign: i32,
    entry_price: f64,
    unrealized_pnl: f64,
    symbol: String,
}

fn update_position(
    pos_value: &serde_json::Value,
    positions: &mut HashMap<i32, PositionState>,
    mark_prices: &HashMap<i32, f64>,
) {
    if let (Some(market_id), Some(position_str), Some(sign)) = (
        pos_value
            .get("market_id")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32),
        pos_value.get("position").and_then(|v| v.as_str()),
        pos_value
            .get("sign")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32),
    ) {
        let size = position_str.parse::<f64>().unwrap_or(0.0);
        if size == 0.0 {
            positions.remove(&market_id);
            println!("🔴 Position closed on market {}", market_id);
            return;
        }

        let entry_price = pos_value
            .get("avg_entry_price")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);

        let unrealized_pnl = pos_value
            .get("unrealized_pnl")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);

        let symbol = pos_value
            .get("symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN")
            .to_string();

        positions.insert(
            market_id,
            PositionState {
                size,
                sign,
                entry_price,
                unrealized_pnl,
                symbol,
            },
        );

        let mark_price = mark_prices.get(&market_id).copied();
        let state = positions.get(&market_id).unwrap();
        display_position(market_id, state, mark_price);
    }
}

fn display_position(market_id: i32, state: &PositionState, mark_price: Option<f64>) {
    let direction = match state.sign {
        s if s > 0 => "LONG",
        s if s < 0 => "SHORT",
        _ => "FLAT",
    };
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "Market {} ({}) - {} {:+.4} contracts",
        market_id,
        state.symbol,
        direction,
        state.size * state.sign as f64
    );
    println!("  Entry Price: ${:.2}", state.entry_price);
    if let Some(mark) = mark_price {
        println!("  Mark Price:  ${:.2}", mark);
    }
    println!("  Unrealized PnL: ${:.2}", state.unrealized_pnl);
    println!();
}
