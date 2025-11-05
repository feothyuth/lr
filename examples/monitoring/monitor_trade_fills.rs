// Monitor Trade Fills in Real-Time
// Subscribes to account_all_trades channel to track order executions
// Shows: fill time, side, size, price, fee, PnL

use anyhow::Result;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::time::{timeout, Duration};

#[path = "../common/example_context.rs"]
mod common;
use common::ExampleContext;
use lighter_client::ws_client::WsEvent;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TradeData {
    trade_id: Option<String>,
    order_id: Option<String>,
    market_id: Option<u32>,
    symbol: Option<String>,
    side: Option<String>, // "buy" or "sell"
    price: Option<String>,
    size: Option<String>,
    fee: Option<String>,
    realized_pnl: Option<String>,
    timestamp: Option<String>,
    is_maker: Option<bool>,
}

#[derive(Debug, Clone)]
struct Fill {
    trade_id: String,
    market_id: u32,
    symbol: String,
    side: String,
    price: f64,
    size: f64,
    fee: f64,
    realized_pnl: f64,
    is_maker: bool,
    timestamp: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt().with_env_filter("info").init();

    println!("\n{}", "═".repeat(80));
    println!("✅ Real-Time Trade Fills Monitor");
    println!("{}\n", "═".repeat(80));

    // Initialize example context
    let ctx = ExampleContext::initialise(Some("monitor_trade_fills")).await?;
    let account_id = ctx.account_id();

    println!("✅ Context initialized\n");

    // Build WebSocket connection with private channel subscription
    println!("📡 Subscribing to account_all_trades...");
    let mut stream = ctx
        .ws_builder()
        .subscribe_account_all_trades(account_id)
        .connect()
        .await?;

    // Set authentication token for private channels
    let auth_token = ctx.auth_token().await?;
    stream.connection_mut().set_auth_token(auth_token);

    println!("✅ Subscribed\n");

    println!("{}", "═".repeat(80));
    println!("🔴 LIVE - Monitoring trade fills (Ctrl+C to stop)");
    println!("{}\n", "═".repeat(80));

    let mut fills: Vec<Fill> = Vec::new();
    let mut total_volume = 0.0;
    let mut total_fees = 0.0;
    let mut total_pnl = 0.0;

    // Listen for fill messages
    loop {
        match timeout(Duration::from_secs(30), stream.next()).await {
            Ok(Some(Ok(event))) => {
                match event {
                    WsEvent::Connected => {
                        println!("✅ Connected to WebSocket");
                    }
                    WsEvent::Pong => {
                        // Heartbeat received
                    }
                    WsEvent::Account(envelope) => {
                        // Parse the account event to find trades
                        let event_value = envelope.event.as_value();

                        // Trades can be in "trades" array or single "trade" object
                        if let Some(trades_array) =
                            event_value.get("trades").and_then(|v| v.as_array())
                        {
                            for trade_value in trades_array {
                                if let Ok(trade_data) =
                                    serde_json::from_value::<TradeData>(trade_value.clone())
                                {
                                    process_fill(
                                        &mut fills,
                                        trade_data,
                                        &mut total_volume,
                                        &mut total_fees,
                                        &mut total_pnl,
                                    );
                                }
                            }
                        } else if let Some(trade_value) = event_value.get("trade") {
                            if let Ok(trade_data) =
                                serde_json::from_value::<TradeData>(trade_value.clone())
                            {
                                process_fill(
                                    &mut fills,
                                    trade_data,
                                    &mut total_volume,
                                    &mut total_fees,
                                    &mut total_pnl,
                                );
                            }
                        }
                    }
                    WsEvent::Closed(frame) => {
                        println!("\n❌ WebSocket connection closed: {:?}", frame);
                        break;
                    }
                    WsEvent::Unknown(raw) => {
                        // Skip unknown events
                        if raw.contains("\"type\":\"connected\"")
                            || raw.contains("\"type\":\"subscribed\"")
                        {
                            continue;
                        }
                    }
                    _ => {
                        // Ignore other event types
                    }
                }
            }
            Ok(Some(Err(e))) => {
                println!("\n❌ WebSocket error: {}", e);
                break;
            }
            Ok(None) => {
                println!("\n❌ WebSocket connection closed");
                break;
            }
            Err(_) => {
                // Timeout - show summary
                println!("\n💓 Heartbeat - {} fills received", fills.len());
                if !fills.is_empty() {
                    println!("   Total Volume: {:.4} contracts", total_volume);
                    println!("   Total Fees:   ${:.4}", total_fees);
                    println!("   Total PnL:    ${:.4}", total_pnl);
                }
                println!();
                continue;
            }
        }
    }

    // Final summary
    println!("\n{}", "═".repeat(80));
    println!("📊 SESSION SUMMARY");
    println!("{}", "═".repeat(80));
    println!("Total Fills:  {}", fills.len());
    println!("Total Volume: {:.4} contracts", total_volume);
    println!("Total Fees:   ${:.4}", total_fees);
    println!("Total PnL:    ${:.4}", total_pnl);
    println!("{}\n", "═".repeat(80));

    Ok(())
}

fn process_fill(
    fills: &mut Vec<Fill>,
    trade_data: TradeData,
    total_volume: &mut f64,
    total_fees: &mut f64,
    total_pnl: &mut f64,
) {
    let trade_id = trade_data
        .trade_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let market_id = trade_data.market_id.unwrap_or(0);
    let symbol = trade_data
        .symbol
        .clone()
        .unwrap_or_else(|| format!("MARKET_{}", market_id));
    let side = trade_data
        .side
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let price: f64 = trade_data
        .price
        .as_ref()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0.0);

    let size: f64 = trade_data
        .size
        .as_ref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    let fee: f64 = trade_data
        .fee
        .as_ref()
        .and_then(|f| f.parse().ok())
        .unwrap_or(0.0);

    let realized_pnl: f64 = trade_data
        .realized_pnl
        .as_ref()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0.0);

    let is_maker = trade_data.is_maker.unwrap_or(false);
    let timestamp = trade_data
        .timestamp
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    // Update totals
    *total_volume += size;
    *total_fees += fee;
    *total_pnl += realized_pnl;

    // Store fill
    let fill = Fill {
        trade_id: trade_id.clone(),
        market_id,
        symbol: symbol.clone(),
        side: side.clone(),
        price,
        size,
        fee,
        realized_pnl,
        is_maker,
        timestamp: timestamp.clone(),
    };

    fills.push(fill);

    // Display fill
    let side_indicator = if side == "buy" {
        "🟢 BUY "
    } else {
        "🔴 SELL"
    };
    let maker_indicator = if is_maker { "M" } else { "T" };
    let pnl_indicator = if realized_pnl >= 0.0 { "🟢" } else { "🔴" };

    println!("\n{}", "─".repeat(80));
    println!("✅ FILL #{} - {}-PERP", fills.len(), symbol);
    println!("{}", "─".repeat(80));
    println!("  {} {} @ ${:.2}", side_indicator, size, price);
    println!("  Notional:  ${:.2}", price * size);
    println!("  Fee:       ${:.4} ({})", fee, maker_indicator);
    println!("  Realized:  {}${:.4}", pnl_indicator, realized_pnl);
    println!("  Time:      {}", timestamp);
    println!("  Trade ID:  {}...", &trade_id[..trade_id.len().min(12)]);
}
