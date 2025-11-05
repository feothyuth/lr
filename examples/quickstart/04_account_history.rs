//! Explores authenticated history endpoints for orders, trades, and withdrawals.
//!
//! Required environment variables:
//! - `LIGHTER_PRIVATE_KEY`
//! - `LIGHTER_ACCOUNT_INDEX`
//! - `LIGHTER_API_KEY_INDEX`
//! Optional:
//! - `LIGHTER_API_URL`
//! - `LIGHTER_MARKET_ID`
//! - `config.toml` / `LIGHTER_CONFIG_PATH` to override non-secret defaults

#[path = "../common/env.rs"]
mod common_env;

#[path = "../common/config.rs"]
mod common_config;

use common_config::get_i64 as config_i64;
use common_env::{ensure_positive, parse_env, require_env, require_parse_env, resolve_api_url};
use lighter_client::lighter_client::{
    HistoryFilter, HistoryQuery, LighterClient, SortDir, TradeSort, TradesQuery,
};
use lighter_client::types::{AccountId, ApiKeyIndex, MarketId};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_url = resolve_api_url("https://mainnet.zklighter.elliot.ai");
    let private_key = require_env(&["LIGHTER_PRIVATE_KEY"], "LIGHTER_PRIVATE_KEY");
    let account_index = ensure_positive(
        require_parse_env(
            &[
                "LIGHTER_ACCOUNT_INDEX",
                "ACCOUNT_INDEX",
                "LIGHTER_ACCOUNT_ID",
            ],
            "account index",
        ),
        "account index",
    );
    let api_key_index = ApiKeyIndex::new(require_parse_env(
        &["LIGHTER_API_KEY_INDEX", "API_KEY_INDEX"],
        "API key index",
    ));
    let market_id = parse_env::<i32>(&["LIGHTER_MARKET_ID"]).or_else(|| {
        config_i64(&["account_history.market_id", "defaults.market_id"])
            .map(|value| i32::try_from(value).expect("market_id must fit in i32"))
    });
    let market = MarketId::new(market_id.unwrap_or(0));

    let client = LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(api_key_index)
        .build()
        .await?;

    let trades = client
        .account()
        .trades(
            TradesQuery::new(TradeSort::Timestamp, 20)?
                .market(market)
                .direction(SortDir::Desc),
        )
        .await?;
    if let Some(fill) = trades.trades.first() {
        println!(
            "Retrieved {} recent fills; newest trade id: {}",
            trades.trades.len(),
            fill.trade_id
        );
    } else {
        println!("No fills returned for market {market}");
    }

    let withdraws = client
        .account()
        .withdraw_history(HistoryQuery::new().filter(HistoryFilter::All))
        .await?;
    println!(
        "You have {} completed withdrawal(s)",
        withdraws.withdraws.len()
    );

    let next_nonce = client.account().next_nonce(api_key_index).await?;
    println!(
        "Next available nonce for API key {} is {}",
        api_key_index.into_inner(),
        next_nonce.nonce
    );

    Ok(())
}
