//! Collects public market data without requiring credentials.
//! Optional environment variables:
//! - `LIGHTER_API_URL`
//! - `LIGHTER_MARKET_ID`
//! Optional configuration:
//! - Set `LIGHTER_CONFIG_PATH` or edit `config.toml` to override defaults.

#[path = "../common/config.rs"]
mod common_config;

use common_config::get_i64 as config_i64;

use lighter_client::lighter_client::{CandleResolution, LighterClient, TimeRange, Timestamp};
use lighter_client::types::MarketId;
use time::{Duration, OffsetDateTime};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_url = std::env::var("LIGHTER_API_URL")
        .unwrap_or_else(|_| "https://mainnet.zklighter.elliot.ai".to_string());
    let market = MarketId::new(load_market_id());

    let client = LighterClient::new(api_url).await?;

    let order_book = client.orders().book(market, 25).await?;
    if let (Some(bid), Some(ask)) = (order_book.bids.first(), order_book.asks.first()) {
        println!(
            "Market {} best bid/ask: {} / {}",
            market, bid.price, ask.price
        );
    } else {
        println!("Order book for market {market} was empty");
    }

    let end = OffsetDateTime::now_utc();
    let start = end - Duration::hours(1);
    let range = TimeRange::new(
        Timestamp::try_from(start.unix_timestamp())?,
        Timestamp::try_from(end.unix_timestamp())?,
    )?;

    let candles = client
        .candles()
        .price(market, CandleResolution::FiveMinutes, range, 12, Some(true))
        .await?;
    println!(
        "Fetched {} five-minute candles for the last hour",
        candles.candlesticks.len()
    );

    let stats = client.orders().exchange_stats().await?;
    println!(
        "Exchange 24h USD volume: {:.2} across {} trades",
        stats.daily_usd_volume, stats.daily_trades_count
    );

    Ok(())
}

fn load_market_id() -> i32 {
    std::env::var("LIGHTER_MARKET_ID")
        .ok()
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            config_i64(&["market_data.market_id", "defaults.market_id"])
                .map(|value| i32::try_from(value).expect("market_id must fit in i32"))
        })
        .unwrap_or(0)
}
