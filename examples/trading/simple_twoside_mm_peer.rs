//! Two-sided maker using peer top-of-book mid (skips our own quotes) with fixed spread.

#[path = "twoside_shared.rs"]
mod shared;

use anyhow::Result;
use shared::{run_two_sided, EngineConfig, QuoteStrategy, StrategyInput, StrategyOutput};

const MARKET_ID: i32 = 1;
const ORDER_SIZE: f64 = 0.0002;
const HALF_SPREAD_PCT: f64 = 0.000025;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    println!("══════════════════════════════════════════════════════════════");
    println!("  TWO-SIDED MAKER – PEER TOP-OF-BOOK CENTER");
    println!("══════════════════════════════════════════════════════════════\n");

    let strategy = PeerMidStrategy {
        half_spread_pct: HALF_SPREAD_PCT,
        last_quotes: None,
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

struct PeerMidStrategy {
    half_spread_pct: f64,
    last_quotes: Option<(f64, f64)>,
}

impl QuoteStrategy for PeerMidStrategy {
    fn compute(&mut self, input: StrategyInput<'_>) -> Option<StrategyOutput> {
        let bid0 = input.view.book.bids.get(0)?;
        let ask0 = input.view.book.asks.get(0)?;
        let raw_bid = bid0.price.parse::<f64>().ok()?;
        let raw_ask = ask0.price.parse::<f64>().ok()?;
        if raw_ask <= raw_bid {
            return None;
        }

        let tick_eps = input.tick_size * 0.51;
        let mut peer_bid = raw_bid;
        let mut peer_ask = raw_ask;

        if let Some((my_bid, my_ask)) = self.last_quotes {
            if (peer_bid - my_bid).abs() <= tick_eps {
                if let Some(level) = input.view.book.bids.get(1) {
                    if let Ok(price) = level.price.parse::<f64>() {
                        peer_bid = price;
                    }
                }
            }
            if (peer_ask - my_ask).abs() <= tick_eps {
                if let Some(level) = input.view.book.asks.get(1) {
                    if let Ok(price) = level.price.parse::<f64>() {
                        peer_ask = price;
                    }
                }
            }
        }

        let center = 0.5 * (peer_bid + peer_ask);
        Some(StrategyOutput {
            center_price: center,
            half_spread_pct: self.half_spread_pct,
            label: "peer_mid",
            diagnostics: vec![
                ("raw_bid", raw_bid),
                ("raw_ask", raw_ask),
                ("peer_bid", peer_bid),
                ("peer_ask", peer_ask),
            ],
        })
    }

    fn record_quote(&mut self, bid_price: f64, ask_price: f64) {
        self.last_quotes = Some((bid_price, ask_price));
    }
}
