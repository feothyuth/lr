//! Subscribes to an order book stream using the websocket helper.
//!
//! Optional environment variables:
//! - `LIGHTER_API_URL`
//! - `LIGHTER_MARKET_ID`
//! Optional configuration via `config.toml` or `LIGHTER_CONFIG_PATH`.

#[path = "../common/config.rs"]
mod common_config;

use common_config::get_i64 as config_i64;

use futures_util::StreamExt;
use lighter_client::{lighter_client::LighterClient, types::MarketId, ws_client::WsEvent};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_url = std::env::var("LIGHTER_API_URL")
        .unwrap_or_else(|_| "https://mainnet.zklighter.elliot.ai".to_string());
    let market = MarketId::new(load_market_id());

    let client = LighterClient::new(api_url).await?;

    let mut stream = client.ws().subscribe_order_book(market).connect().await?;

    println!("Connected; waiting for order book snapshots...");
    let mut updates = 0usize;
    while let Some(event) = stream.next().await {
        match event? {
            WsEvent::Connected => println!("Stream handshake complete"),
            WsEvent::OrderBook(update) => {
                let best_bid = update.state.bids.first().map(|level| level.price.clone());
                let best_ask = update.state.asks.first().map(|level| level.price.clone());
                println!(
                    "Order book {} update: bid={:?} ask={:?}",
                    update.market, best_bid, best_ask
                );
                updates += 1;
                if updates >= 3 {
                    println!("Received three updates; closing stream.");
                    break;
                }
            }
            WsEvent::Closed(frame) => {
                println!("Stream closed: {:?}", frame);
                break;
            }
            other => println!("Unhandled event: {:?}", other),
        }
    }

    Ok(())
}

fn load_market_id() -> i32 {
    std::env::var("LIGHTER_MARKET_ID")
        .ok()
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            config_i64(&["websocket.market_id", "defaults.market_id"])
                .map(|value| i32::try_from(value).expect("market_id must fit in i32"))
        })
        .unwrap_or(0)
}
