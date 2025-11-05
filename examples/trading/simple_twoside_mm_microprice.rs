//! Two-sided maker centred on microprice (depth-weighted mid) with a tight clamp.

#[path = "twoside_shared.rs"]
mod shared;

use anyhow::Result;
use lighter_client::ws_client::OrderBookLevel;
use shared::{run_two_sided, EngineConfig, QuoteStrategy, StrategyInput, StrategyOutput};

const MARKET_ID: i32 = 1;
const ORDER_SIZE: f64 = 0.0002;
const HALF_SPREAD_PCT: f64 = 0.000025;
const CLAMP_TICKS: i64 = 2;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    println!("══════════════════════════════════════════════════════════════");
    println!("  TWO-SIDED MAKER – MICROPRICE CENTER");
    println!("══════════════════════════════════════════════════════════════\n");

    let strategy = MicropriceStrategy {
        half_spread_pct: HALF_SPREAD_PCT,
        clamp_ticks: CLAMP_TICKS,
    };

    run_two_sided(
        EngineConfig {
            market_id: MARKET_ID,
            order_size: ORDER_SIZE,
            default_half_spread_pct: HALF_SPREAD_PCT,
            dry_run: false,
        },
        strategy,
    )
    .await
}

struct MicropriceStrategy {
    half_spread_pct: f64,
    clamp_ticks: i64,
}

impl QuoteStrategy for MicropriceStrategy {
    fn compute(&mut self, input: StrategyInput<'_>) -> Option<StrategyOutput> {
        let (bid, bid_qty) = level_values(input.view.book.bids.first()?)?;
        let (ask, ask_qty) = level_values(input.view.book.asks.first()?)?;
        if ask <= bid {
            return None;
        }

        let mid = 0.5 * (bid + ask);
        let total = bid_qty + ask_qty;
        let mut micro = if total > f64::EPSILON {
            (ask * bid_qty + bid * ask_qty) / total
        } else {
            mid
        };

        let clamp = self.clamp_ticks as f64 * input.tick_size;
        micro = micro.clamp(mid - clamp, mid + clamp);

        Some(StrategyOutput {
            center_price: micro,
            half_spread_pct: self.half_spread_pct,
            label: "micro_mid",
            diagnostics: vec![("mid", mid), ("micro", micro)],
        })
    }
}

fn level_values(level: &OrderBookLevel) -> Option<(f64, f64)> {
    let price = level.price.parse::<f64>().ok()?;
    let size = level
        .size
        .parse::<f64>()
        .ok()
        .or_else(|| level.remaining_base_amount.as_ref()?.parse::<f64>().ok())
        .unwrap_or(0.0);
    Some((price, size))
}
