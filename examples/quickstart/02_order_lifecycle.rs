//! Shows how to prepare, sign, and optionally submit a limit order.
//!
//! Required environment variables:
//! - `LIGHTER_PRIVATE_KEY`
//! - `LIGHTER_ACCOUNT_INDEX`
//! - `LIGHTER_API_KEY_INDEX`
//! Optional overrides:
//! - `LIGHTER_API_URL`
//! - `LIGHTER_MARKET_ID`
//! - `LIGHTER_ORDER_QTY`
//! - `LIGHTER_LIMIT_TICKS`

#[path = "../common/env.rs"]
mod common_env;

use common_env::{ensure_positive, parse_env_or, require_env, require_parse_env, resolve_api_url};
use lighter_client::{
    lighter_client::{LighterClient, OrderBuilder, OrderStateReady},
    types::{AccountId, ApiKeyIndex, BaseQty, Expiry, MarketId, Price},
};
use time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = build_client().await?;
    let market = MarketId::new(parse_env_or(&["LIGHTER_MARKET_ID"], 0));
    let quantity = parse_env_or(&["LIGHTER_ORDER_QTY"], 1_00);
    let limit_ticks = parse_env_or(&["LIGHTER_LIMIT_TICKS"], 410_000);

    // PostOnly/GTT orders must expire at least 10 minutes in the future.
    // Use 15 minutes to satisfy server-side validation with margin.
    let signed = limit_order(&client, market, quantity, limit_ticks)
        .expires_at(Expiry::from_now(Duration::minutes(15)))
        .post_only()
        .sign()
        .await?;

    println!("Signed limit order payload: {}", signed.payload());

    // Uncomment to submit the live order:
    // let submission = limit_order(&client, market, quantity, limit_ticks)
    //     .expires_at(Expiry::from_now(Duration::minutes(15)))
    //     .post_only()
    //     .submit()
    //     .await?;
    // println!("Order accepted with tx hash {}", submission.response().tx_hash);

    Ok(())
}

fn limit_order<'a>(
    client: &'a LighterClient,
    market: MarketId,
    quantity: i64,
    limit_ticks: i64,
) -> OrderBuilder<'a, OrderStateReady> {
    let qty = BaseQty::try_from(quantity).expect("quantity must be non-zero");
    client
        .order(market)
        .buy()
        .qty(qty)
        .limit(Price::ticks(limit_ticks))
}

async fn build_client() -> Result<LighterClient, Box<dyn std::error::Error>> {
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
    let api_key_index =
        require_parse_env(&["LIGHTER_API_KEY_INDEX", "API_KEY_INDEX"], "API key index");

    Ok(LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?)
}
