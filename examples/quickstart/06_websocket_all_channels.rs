//! Demonstrates subscribing to all WebSocket channels including private channels.
//!
//! This example shows how to use the enhanced WebSocket client to subscribe to:
//! - Public channels: order books, trades, BBO, market stats, transactions, height
//! - Private channels: positions, orders, trades, user stats, account transactions
//!
//! Required environment variables:
//! - `LIGHTER_PRIVATE_KEY` - Your private key for authentication
//! - `LIGHTER_API_URL` (optional) - defaults to mainnet
//! - `LIGHTER_MARKET_ID` (optional) - defaults to 0 (ETH)
//! - `LIGHTER_ACCOUNT_ID` (optional) - defaults to 0

use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, MarketId},
    ws_client::WsEvent,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_url = std::env::var("LIGHTER_API_URL")
        .unwrap_or_else(|_| "https://mainnet.zklighter.elliot.ai".to_string());
    let market = MarketId::new(env_or_default("LIGHTER_MARKET_ID", 0));
    let account = AccountId::new(env_or_default("LIGHTER_ACCOUNT_ID", 0));

    println!("🚀 Connecting to Lighter DEX WebSocket");
    println!("📊 Market ID: {}", market.0);
    println!("👤 Account ID: {}", account.0);

    let client = LighterClient::new(api_url).await?;

    // Build WebSocket connection with multiple channel subscriptions
    let mut stream = client
        .ws()
        // Public market data channels
        .subscribe_order_book(market)
        .subscribe_account(account)
        .subscribe_bbo(market)
        .subscribe_trade(market)
        .subscribe_market_stats(market)
        .subscribe_transactions()
        .subscribe_executed_transactions()
        .subscribe_height()
        // Private account channels (requires authentication)
        .subscribe_account_all_positions(account)
        .subscribe_account_all_orders(account)
        .subscribe_account_all_trades(account)
        .subscribe_user_stats(account)
        .subscribe_account_tx(account)
        // Market-specific private channels
        .subscribe_account_market_orders(market, account)
        .subscribe_account_market_positions(market, account)
        .subscribe_account_market_trades(market, account)
        .connect()
        .await?;

    println!("✅ Connected to WebSocket with 17+ channel subscriptions");
    println!("📡 Listening for events...\n");

    let mut event_count = 0;
    let max_events = 20;

    while let Some(event) = stream.next().await {
        match event? {
            WsEvent::Connected => {
                println!("🔗 WebSocket handshake complete");
            }
            WsEvent::OrderBook(update) => {
                let best_bid = update.state.bids.first().map(|l| &l.price);
                let best_ask = update.state.asks.first().map(|l| &l.price);
                println!(
                    "📖 Order Book Update - Market: {}, Bid: {:?}, Ask: {:?}",
                    update.market, best_bid, best_ask
                );
            }
            WsEvent::Account(account_event) => {
                println!(
                    "👤 Account Event - Account: {}, Snapshot: {}",
                    account_event.account, account_event.snapshot
                );
                println!("   Event data: {:?}", account_event.event);
            }
            WsEvent::Pong => {
                println!("🏓 Pong/Ping handled");
            }
            WsEvent::Closed(frame) => {
                println!("🔌 WebSocket closed: {:?}", frame);
                break;
            }
            other => {
                println!("📨 Other event: {:?}", other);
            }
        }

        event_count += 1;
        if event_count >= max_events {
            println!("\n✅ Received {} events. Closing connection.", max_events);
            break;
        }
    }

    println!("👋 Example completed successfully!");
    Ok(())
}

fn env_or_default<T: std::str::FromStr + Copy>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}
