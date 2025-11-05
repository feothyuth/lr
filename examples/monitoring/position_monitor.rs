//! Position Monitor for Lighter DEX Market Making
//!
//! Real-time monitoring of positions with:
//! - Position size and direction (LONG/SHORT)
//! - Entry price vs current mark price
//! - Unrealized PnL
//! - Live updates via WebSocket

#[path = "../common/example_context.rs"]
mod common;

use futures_util::StreamExt;
use lighter_client::ws_client::WsEvent;
use std::collections::HashMap;

use common::ExampleContext;

#[derive(Debug, Clone)]
struct PositionState {
    size: f64,
    sign: i32,
    entry_price: f64,
    unrealized_pnl: f64,
    allocated_margin: f64,
    position_value: f64,
    initial_margin_fraction: f64,
    symbol: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = ExampleContext::initialise(Some("position_monitor")).await?;
    let client = ctx.client();
    let account_id = ctx.account_id();
    let market = ctx.market_id();
    let account_index = account_id.into_inner();
    let market_id: i32 = market.into();

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║              POSITION MONITOR - LIGHTER DEX                  ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();
    println!("Configuration:");
    println!("  Account Index: {}", account_index);
    println!("  Monitoring Market ID: {}", market_id);
    println!();

    // Track positions by market_id (100% WebSocket - no REST API call)
    let mut positions: HashMap<i32, PositionState> = HashMap::new();

    println!("Using 100% WebSocket - waiting for initial position data...");
    println!();

    // Track mark prices by market_id
    let mut mark_prices: HashMap<i32, f64> = HashMap::new();

    // Subscribe to WebSocket streams
    println!("Connecting to WebSocket streams...");
    let mut stream = client
        .ws()
        .subscribe_account_all_positions(account_id)
        .subscribe_market_stats(market)
        .connect()
        .await?;

    println!("✅ Connected! Listening for position and market updates...");
    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    while let Some(event) = stream.next().await {
        match event? {
            WsEvent::Connected => {
                println!("🔗 WebSocket handshake complete");
            }

            WsEvent::Account(account_event) => {
                // Parse account event for position updates
                let event_data = account_event.event.as_value();

                // Try to extract positions from the event
                if let Some(positions_obj) =
                    event_data.get("positions").and_then(|v| v.as_object())
                {
                    for (market_key, pos_value) in positions_obj {
                        let market_id = pos_value
                            .get("market_id")
                            .and_then(|v| v.as_i64())
                            .map(|v| v as i32)
                            .or_else(|| market_key.parse().ok());

                        let pos_size = pos_value
                            .get("position")
                            .and_then(|v| {
                                v.as_str()
                                    .and_then(|s| s.parse::<f64>().ok())
                                    .or_else(|| v.as_f64())
                            });

                        let sign_opt = pos_value
                            .get("sign")
                            .and_then(|v| v.as_i64())
                            .map(|v| v as i32);

                        if let (Some(market_id), Some(pos_size), Some(sign)) =
                            (market_id, pos_size, sign_opt)
                        {
                            if pos_size == 0.0 {
                                // Position closed
                                if positions.remove(&market_id).is_some() {
                                    println!("🔴 POSITION CLOSED - Market ID: {}", market_id);
                                    println!();
                                }
                            } else {
                                // Position updated or opened
                                let entry_price: f64 = pos_value
                                    .get("avg_entry_price")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0.0);

                                let unrealized_pnl: f64 = pos_value
                                    .get("unrealized_pnl")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0.0);

                                let allocated_margin: f64 = pos_value
                                    .get("allocated_margin")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0.0);

                                let position_value: f64 = pos_value
                                    .get("position_value")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0.0);

                                let initial_margin_fraction: f64 = pos_value
                                    .get("initial_margin_fraction")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0.0);

                                let symbol = pos_value
                                    .get("symbol")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("UNKNOWN")
                                    .to_string();

                                let was_new = !positions.contains_key(&market_id);

                                let state = PositionState {
                                    size: pos_size,
                                    sign,
                                    entry_price,
                                    unrealized_pnl,
                                    allocated_margin,
                                    position_value,
                                    initial_margin_fraction,
                                    symbol,
                                };

                                positions.insert(market_id, state.clone());

                                if was_new {
                                    println!("🟢 NEW POSITION OPENED:");
                                } else {
                                    println!("🔄 POSITION UPDATED:");
                                }

                                let mark_price = mark_prices.get(&market_id).copied();
                                display_position(market_id, &state, mark_price);
                                println!();
                            }
                        }
                    }
                }
            }

            WsEvent::MarketStats(stats) => {
                // Update mark price
                let mark_price: f64 = stats.market_stats.mark_price.parse().unwrap_or(0.0);
                let market_id_i32 = stats.market_stats.market_id as i32;
                mark_prices.insert(market_id_i32, mark_price);

                // Display updated position with new mark price if we have a position for this market
                if let Some(pos) = positions.get(&market_id_i32) {
                    println!("📊 MARK PRICE UPDATE:");
                    display_position(market_id_i32, pos, Some(mark_price));
                    println!();
                }
            }

            WsEvent::Closed(frame) => {
                println!("🔌 WebSocket closed: {:?}", frame);
                break;
            }

            _ => {
                // Ignore other events
            }
        }
    }

    Ok(())
}

fn display_position(market_id: i32, pos: &PositionState, mark_price: Option<f64>) {
    let direction = if pos.sign == 1 { "LONG" } else { "SHORT" };
    let sign_symbol = if pos.sign == 1 { "+" } else { "-" };

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!(
        "║   POSITION MONITOR - {}                                    ",
        pad_right(&pos.symbol, 36)
    );
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();
    println!("Market ID: {}", market_id);
    println!(
        "Position: {}{} ({} {})",
        sign_symbol, pos.size, direction, pos.symbol
    );
    println!();

    if let Some(mark) = mark_price {
        println!("Mark Price: ${:.2}", mark);
    } else {
        println!("Mark Price: (waiting for update...)");
    }

    println!("Entry Price: ${:.2}", pos.entry_price);
    println!();

    // Calculate PnL percentage
    // For isolated margin: use allocated_margin (return on margin)
    // For cross margin: calculate margin from position_value * initial_margin_fraction
    let pnl_pct = if pos.allocated_margin > 0.001 {
        // Isolated margin: PnL% = (unrealized_pnl / allocated_margin) * 100
        (pos.unrealized_pnl / pos.allocated_margin) * 100.0
    } else {
        // Cross margin: Calculate margin used from initial margin requirement
        // initial_margin_fraction is stored as percentage (e.g., 2.0 = 2%)
        let margin_used = pos.position_value * (pos.initial_margin_fraction / 100.0);
        if margin_used > 0.001 {
            (pos.unrealized_pnl / margin_used) * 100.0
        } else {
            0.0
        }
    };

    let pnl_sign = if pos.unrealized_pnl >= 0.0 { "+" } else { "" };
    let pnl_emoji = if pos.unrealized_pnl >= 0.0 {
        "📈"
    } else {
        "📉"
    };

    let leverage = if pos.initial_margin_fraction > 0.0 {
        100.0 / pos.initial_margin_fraction
    } else {
        0.0
    };

    if pos.allocated_margin > 0.001 {
        // Isolated margin mode
        println!(
            "{} Unrealized PnL: {}${:.2} ({}{:.2}% ROI)",
            pnl_emoji,
            pnl_sign,
            pos.unrealized_pnl.abs(),
            pnl_sign,
            pnl_pct
        );
    } else {
        // Cross margin mode
        println!(
            "{} Unrealized PnL: {}${:.2} ({}{:.2}% ROI @ {:.0}x leverage)",
            pnl_emoji,
            pnl_sign,
            pos.unrealized_pnl.abs(),
            pnl_sign,
            pnl_pct,
            leverage
        );
    }

    // If we have mark price, calculate additional metrics
    if let Some(mark) = mark_price {
        let price_diff = mark - pos.entry_price;
        let price_diff_pct = if pos.entry_price > 0.0 {
            (price_diff / pos.entry_price) * 100.0
        } else {
            0.0
        };

        let diff_sign = if price_diff >= 0.0 { "+" } else { "" };
        println!(
            "Price Change: {}${:.2} ({}{:.2}%)",
            diff_sign,
            price_diff.abs(),
            diff_sign,
            price_diff_pct
        );
    }
}

fn pad_right(s: &str, width: usize) -> String {
    if s.len() >= width {
        s[..width].to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - s.len()))
    }
}
