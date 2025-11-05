//! Compare different price sources for debugging
//! Shows: Mark Price, Index Price, Last Price, BBO Mid, Order Book Mid

#[path = "../common/example_context.rs"]
mod common;

use chrono::Local;
use futures_util::StreamExt;
use lighter_client::ws_client::WsEvent;
use std::collections::HashMap;

use common::ExampleContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = ExampleContext::initialise(Some("compare_prices")).await?;
    let client = ctx.client();
    let market = ctx.market_id();
    let market_index: i32 = market.into();

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!(
        "║         PRICE COMPARISON - Market {:>2} (configured)        ║",
        market_index
    );
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    // Store different price types
    let mut prices: HashMap<String, f64> = HashMap::new();

    // Subscribe to multiple channels
    let mut stream = client
        .ws()
        .subscribe_market_stats(market)
        .subscribe_order_book(market)
        .connect()
        .await?;

    println!("Connected to WebSocket. Listening for prices...");
    println!();

    let mut update_count = 0;
    let max_updates = 10;

    while let Some(event) = stream.next().await {
        match event? {
            WsEvent::MarketStats(stats) => {
                let timestamp = Local::now().format("%H:%M:%S%.3f");

                // Parse all available prices
                if let Ok(mark_price) = stats.market_stats.mark_price.parse::<f64>() {
                    prices.insert("mark".to_string(), mark_price);
                    println!("[{}] Mark Price: ${:.2}", timestamp, mark_price);
                }

                if let Ok(index_price) = stats.market_stats.index_price.parse::<f64>() {
                    prices.insert("index".to_string(), index_price);
                    println!("[{}] Index Price: ${:.2}", timestamp, index_price);
                }

                if let Ok(last_price) = stats.market_stats.last_trade_price.parse::<f64>() {
                    prices.insert("last".to_string(), last_price);
                    println!("[{}] Last Traded Price: ${:.2}", timestamp, last_price);
                }

                // Show comparison if we have order book mid too
                if let Some(ob_mid) = prices.get("ob_mid") {
                    println!("[{}] Order Book Mid: ${:.2}", timestamp, ob_mid);

                    if let Some(mark) = prices.get("mark") {
                        let diff = mark - ob_mid;
                        println!("      Difference (Mark - OB Mid): ${:.2}", diff);
                    }
                }

                println!();
                update_count += 1;

                if update_count >= max_updates {
                    break;
                }
            }

            WsEvent::OrderBook(ob_event) => {
                let timestamp = Local::now().format("%H:%M:%S%.3f");

                // Get BBO from order book
                let best_bid = ob_event
                    .state
                    .bids
                    .first()
                    .and_then(|level| level.price.parse::<f64>().ok());
                let best_ask = ob_event
                    .state
                    .asks
                    .first()
                    .and_then(|level| level.price.parse::<f64>().ok());

                if let (Some(bid), Some(ask)) = (best_bid, best_ask) {
                    let ob_mid = (bid + ask) / 2.0;
                    prices.insert("ob_mid".to_string(), ob_mid);

                    println!("[{}] Order Book BBO: ${:.2} x ${:.2}", timestamp, bid, ask);
                    println!("[{}] Order Book Mid: ${:.2}", timestamp, ob_mid);
                    println!();
                }
            }

            WsEvent::Connected => {
                println!("WebSocket connected");
            }

            _ => {}
        }
    }

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║                    FINAL COMPARISON                          ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    for (name, price) in &prices {
        println!("{:20} ${:.2}", format!("{}:", name), price);
    }

    Ok(())
}
