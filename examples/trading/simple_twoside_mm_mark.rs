//! Two-sided maker centred on bounded mark price (fair value) with tick drift limits.

#[path = "twoside_shared.rs"]
mod shared;

use std::time::Duration;

use anyhow::Result;
use lighter_client::ws_client::OrderBookLevel;
use shared::{run_two_sided, EngineConfig, QuoteStrategy, StrategyInput, StrategyOutput};

const MARKET_ID: i32 = 1;
const ORDER_SIZE: f64 = 0.0002;
const HALF_SPREAD_PCT: f64 = 0.000025;
const MARK_FRESH_FOR: Duration = Duration::from_millis(200);
const MAX_DRIFT_TICKS: i64 = 4;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    println!("══════════════════════════════════════════════════════════════");
    println!("  TWO-SIDED MAKER – BOUNDED MARK CENTER");
    println!("══════════════════════════════════════════════════════════════\n");

    let strategy = MarkBoundedStrategy {
        half_spread_pct: HALF_SPREAD_PCT,
        mark_fresh_for: MARK_FRESH_FOR,
        max_drift_ticks: MAX_DRIFT_TICKS,
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

struct MarkBoundedStrategy {
    half_spread_pct: f64,
    mark_fresh_for: Duration,
    max_drift_ticks: i64,
}

impl QuoteStrategy for MarkBoundedStrategy {
    fn compute(&mut self, input: StrategyInput<'_>) -> Option<StrategyOutput> {
        let bid = top_price(input.view.book.bids.first()?)?;
        let ask = top_price(input.view.book.asks.first()?)?;
        if ask <= bid {
            return None;
        }
        let mid = 0.5 * (bid + ask);
        let drift_cap = self.max_drift_ticks as f64 * input.tick_size;

        let (center, mark, age_ms) = match (input.view.mark_price, input.view.mark_timestamp) {
            (Some(mark), Some(ts))
                if input.now.saturating_duration_since(ts) <= self.mark_fresh_for =>
            {
                let bounded = mark.clamp(mid - drift_cap, mid + drift_cap);
                (
                    bounded,
                    Some(mark),
                    Some(input.now.saturating_duration_since(ts).as_millis() as f64),
                )
            }
            _ => (mid, None, None),
        };

        let mut diagnostics = vec![("mid", mid), ("center", center)];
        if let Some(mark) = mark {
            diagnostics.push(("mark", mark));
        }
        if let Some(age) = age_ms {
            diagnostics.push(("mark_age_ms", age));
        }

        Some(StrategyOutput {
            center_price: center,
            half_spread_pct: self.half_spread_pct,
            label: "mark_bound",
            diagnostics,
        })
    }
}

fn top_price(level: &OrderBookLevel) -> Option<f64> {
    level.price.parse::<f64>().ok()
}
