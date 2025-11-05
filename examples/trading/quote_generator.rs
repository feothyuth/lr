//! Quote Generator for Lighter DEX Market Making
//!
//! This example demonstrates:
//! 1. Subscribing to Market Stats WebSocket stream for BTC-PERP
//! 2. Using mark price as the reference price
//! 3. Generating bid/ask quotes with configurable spreads
//! 4. Real-time console output updates on every market stats change
//!
//! Usage:
//! ```bash
//! cargo run --example quote_generator
//! ```

use futures_util::StreamExt;
use lighter_client::{lighter_client::LighterClient, types::MarketId, ws_client::WsEvent};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║         QUOTE GENERATOR - LIGHTER DEX MARKET MAKING          ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    // Configuration
    let api_url = "https://mainnet.zklighter.elliot.ai";
    let market_id = 1; // BTC-PERP
    let market = MarketId::new(market_id);

    // Configurable spreads (as decimal percentages)
    let spreads = vec![0.001, 0.002, 0.005]; // 0.1%, 0.2%, 0.5%

    println!("Configuration:");
    println!("  API URL: {}", api_url);
    println!("  Market: BTC-PERP (ID: {})", market_id);
    println!("  Spreads: 0.1%, 0.2%, 0.5%");
    println!();
    println!("Connecting to WebSocket...");

    // Initialize client and connect to WebSocket
    let client = LighterClient::new(api_url).await?;
    let mut stream = client.ws().subscribe_market_stats(market).connect().await?;

    println!("Connected! Listening for Market Stats updates...");
    println!();
    println!("═══════════════════════════════════════════════════════════════");
    println!();

    // Listen for Market Stats events
    while let Some(event) = stream.next().await {
        match event? {
            WsEvent::Connected => {
                println!("WebSocket handshake complete");
            }
            WsEvent::MarketStats(stats) => {
                // Parse mark price
                if let Ok(mark_price) = stats.market_stats.mark_price.parse::<f64>() {
                    // Display mark price
                    println!("Mark Price: ${:.2}", mark_price);
                    println!();

                    // Generate and display quotes for each spread
                    println!("Quotes:");
                    for spread in &spreads {
                        let spread_percentage = spread * 100.0;
                        let bid_quote = mark_price * (1.0 - spread);
                        let ask_quote = mark_price * (1.0 + spread);

                        println!(
                            "  {:.1}% spread: BID ${:.2} | ASK ${:.2}",
                            spread_percentage, bid_quote, ask_quote
                        );
                    }

                    println!();
                    println!("───────────────────────────────────────────────────────────────");
                    println!();
                } else {
                    println!("Warning: Failed to parse mark price");
                }
            }
            WsEvent::Pong => {
                // Ping/pong for connection health - silent
            }
            WsEvent::Closed(frame) => {
                println!("WebSocket closed: {:?}", frame);
                break;
            }
            _ => {
                // Ignore other event types
            }
        }
    }

    println!();
    println!("Quote generator stopped.");

    Ok(())
}
