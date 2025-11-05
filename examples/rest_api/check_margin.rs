#[path = "../common/env.rs"]
mod env;

use env::resolve_api_url;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex},
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
        .api_url(resolve_api_url("https://mainnet.zklighter.elliot.ai"))
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    let details = client.account().details().await?;

    if let Some(account) = details.accounts.first() {
        for pos in &account.positions {
            let pos_size: f64 = pos.position.parse().unwrap_or(0.0);
            if pos_size != 0.0 {
                println!("Market: {} ({})", pos.market_id, pos.symbol);
                println!("  Position: {}", pos.position);
                println!("  Sign: {}", pos.sign);
                println!("  Entry Price: {}", pos.avg_entry_price);
                println!("  Unrealized PnL: {}", pos.unrealized_pnl);
                println!("  Allocated Margin: {}", pos.allocated_margin);
                println!("  Position Value: {}", pos.position_value);
                println!("  Initial Margin Fraction: {}", pos.initial_margin_fraction);

                // Calculate what margin WOULD be used
                let position_value: f64 = pos.position_value.parse().unwrap_or(0.0);
                let initial_margin_fraction: f64 =
                    pos.initial_margin_fraction.parse().unwrap_or(0.0);
                let margin_used = position_value * initial_margin_fraction;
                println!("  Calculated Margin Used: ${:.6}", margin_used);

                // Calculate PnL%
                let unrealized_pnl: f64 = pos.unrealized_pnl.parse().unwrap_or(0.0);
                if margin_used > 0.0 {
                    let pnl_pct = (unrealized_pnl / margin_used) * 100.0;
                    println!("  PnL% (on margin): {:.2}%", pnl_pct);
                }
                println!();
            }
        }
    }

    Ok(())
}
