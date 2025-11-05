//! Test to verify ping/pong is working
//! Logs ALL WebSocket events including pings/pongs

use chrono::Local;
use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, MarketId},
    ws_client::WsEvent,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let private_key = std::env::var("LIGHTER_PRIVATE_KEY").expect("LIGHTER_PRIVATE_KEY not set");
    let account_index: i64 = std::env::var("ACCOUNT_INDEX")
        .expect("ACCOUNT_INDEX not set")
        .parse()?;
    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")
        .unwrap_or_else(|_| "0".to_string())
        .parse()?;

    let client = LighterClient::builder()
        .api_url("https://mainnet.zklighter.elliot.ai")
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    let market = MarketId::new(1);

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║         PING/PONG TEST - LIGHTER DEX                         ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();
    println!("Connecting to WebSocket...");
    println!("Will log ALL events including ping/pong");
    println!();

    let mut stream = client.ws().subscribe_market_stats(market).connect().await?;

    println!("✅ Connected! Waiting for events...");
    println!();

    let mut event_count = 0;
    let max_events = 500; // Run longer to catch pings

    while let Some(event) = stream.next().await {
        let timestamp = Local::now().format("%H:%M:%S%.3f");

        match event? {
            WsEvent::Connected => {
                println!("[{}] 🔗 WebSocket Connected", timestamp);
            }
            WsEvent::Pong => {
                println!(
                    "[{}] 🏓 PONG (SDK auto-responded to server ping)",
                    timestamp
                );
            }
            WsEvent::MarketStats(stats) => {
                println!(
                    "[{}] 📊 Market Stats (mark: ${})",
                    timestamp, stats.market_stats.mark_price
                );
                event_count += 1;
            }
            WsEvent::Closed(frame) => {
                println!("[{}] 🔌 WebSocket Closed: {:?}", timestamp, frame);
                break;
            }
            WsEvent::Unknown(msg) => {
                if msg.contains("ping") || msg.contains("pong") {
                    println!("[{}] 🔍 UNKNOWN (contains ping/pong): {}", timestamp, msg);
                } else if msg.len() < 200 {
                    println!("[{}] ❓ Unknown event: {}", timestamp, msg);
                } else {
                    println!("[{}] ❓ Unknown event ({}  chars)", timestamp, msg.len());
                }
            }
            other => {
                println!("[{}] ❓ Other event: {:?}", timestamp, other);
            }
        }

        if event_count >= max_events {
            println!();
            println!("Received {} data events. Test complete.", max_events);
            break;
        }
    }

    println!();
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║                    TEST COMPLETE                             ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");

    Ok(())
}
