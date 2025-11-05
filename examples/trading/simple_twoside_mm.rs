//! Simple two-sided maker quoting a fixed spread around the raw order-book mid.

#[path = "twoside_shared.rs"]
mod shared;

use anyhow::Result;
use lighter_client::ws_client::OrderBookLevel;
use shared::{run_two_sided, EngineConfig, QuoteStrategy, StrategyInput, StrategyOutput};

const MARKET_ID: i32 = 1;
const ORDER_SIZE: f64 = 0.0002;
const HALF_SPREAD_PCT: f64 = 0.000025;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    println!("══════════════════════════════════════════════════════════════");
    println!("  TWO-SIDED MAKER – RAW MID CENTER");
    println!("══════════════════════════════════════════════════════════════\n");

    let strategy = RawMidStrategy {
        half_spread_pct: HALF_SPREAD_PCT,
    };

    run_two_sided(
        EngineConfig {
            market_id: MARKET_ID,
            order_size: ORDER_SIZE,
            default_half_spread_pct: HALF_SPREAD_PCT,
            dry_run: true,
        },
        strategy,
    )
    .await
}

struct RawMidStrategy {
    half_spread_pct: f64,
}

impl QuoteStrategy for RawMidStrategy {
    fn compute(&mut self, input: StrategyInput<'_>) -> Option<StrategyOutput> {
        let bid = top_price(input.view.book.bids.first()?)?;
        let ask = top_price(input.view.book.asks.first()?)?;
        if ask <= bid {
            return None;
        }
        let center = 0.5 * (bid + ask);
        Some(StrategyOutput {
            center_price: center,
            half_spread_pct: self.half_spread_pct,
            label: "ob_mid",
            diagnostics: Vec::new(),
        })
    }
}

fn top_price(level: &OrderBookLevel) -> Option<f64> {
    level.price.parse::<f64>().ok()
}
