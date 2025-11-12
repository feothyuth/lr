use crate::ws_client::OrderBookState;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct MarketTick {
    pub bid: f64,
    pub ask: f64,
    pub mid: f64,
    pub spread: f64,
    pub timestamp: Instant,
}

#[derive(Clone, Debug)]
pub struct MarketDataState {
    alpha: f64,
    smoothed_mid: Option<f64>,
    last_tick: Option<MarketTick>,
}

impl MarketDataState {
    pub fn new(alpha: f64) -> Self {
        Self {
            alpha,
            smoothed_mid: None,
            last_tick: None,
        }
    }

    pub fn on_order_book(
        &mut self,
        book: &OrderBookState,
        timestamp: Instant,
    ) -> Option<MarketTick> {
        let top_bid = book.bids.first()?.price.parse::<f64>().ok()?;
        let top_ask = book.asks.first()?.price.parse::<f64>().ok()?;
        if top_ask <= top_bid {
            return None;
        }
        let mid = 0.5 * (top_bid + top_ask);
        let smoothed_mid = match self.smoothed_mid {
            Some(prev) => self.alpha * mid + (1.0 - self.alpha) * prev,
            None => mid,
        };
        self.smoothed_mid = Some(smoothed_mid);
        let spread = top_ask - top_bid;
        let tick = MarketTick {
            bid: top_bid,
            ask: top_ask,
            mid: smoothed_mid,
            spread,
            timestamp,
        };
        self.last_tick = Some(tick.clone());
        Some(tick)
    }

    pub fn last_tick(&self) -> Option<&MarketTick> {
        self.last_tick.as_ref()
    }
}
