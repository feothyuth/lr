use crate::avellaneda::market_data::MarketTick;
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Core strategy parameters derived from configuration.
#[derive(Clone, Debug)]
pub struct StrategyParams {
    pub gamma: f64,
    pub kappa: f64,
    pub order_size: f64,
    pub time_horizon_hours: f64,
    pub start_time: Instant,
}

impl StrategyParams {
    pub fn new(gamma: f64, kappa: f64, order_size: f64, time_horizon_hours: f64) -> Self {
        Self {
            gamma,
            kappa,
            order_size,
            time_horizon_hours,
            start_time: Instant::now(),
        }
    }

    pub fn time_left_seconds(&self) -> f64 {
        let total = self.time_horizon_hours * 3600.0;
        let elapsed = self.start_time.elapsed().as_secs_f64();
        (total - elapsed).max(0.01)
    }

    pub fn reset_start_time(&mut self) {
        self.start_time = Instant::now();
    }
}

/// Individual quote order produced by the strategy.
#[derive(Clone, Debug)]
pub struct QuoteOrder {
    pub price: f64,
    pub size: f64,
    pub label: &'static str,
}

impl QuoteOrder {
    pub fn new(price: f64, size: f64, label: &'static str) -> Self {
        Self { price, size, label }
    }
}

/// Bid/Ask pair derived from the Avellaneda-Stoikov formulas.
#[derive(Clone, Debug)]
pub struct QuotePair {
    pub bid: QuoteOrder,
    pub ask: QuoteOrder,
    pub reservation_price: f64,
    pub optimal_spread: f64,
}

impl QuotePair {
    pub fn new(
        bid: QuoteOrder,
        ask: QuoteOrder,
        reservation_price: f64,
        optimal_spread: f64,
    ) -> Self {
        Self {
            bid,
            ask,
            reservation_price,
            optimal_spread,
        }
    }
}

/// Extended context for a quote computation.
#[derive(Clone, Debug, Default, Serialize)]
pub struct QuoteContext {
    pub reservation_price: f64,
    pub raw_spread: f64,
    pub raw_spread_bps: f64,
    pub effective_spread: f64,
    pub effective_spread_bps: f64,
    pub min_required_spread_bps: f64,
    pub maker_fee_bps: f64,
}

/// Snapshot of inventory state used for diagnostics.
#[derive(Clone, Debug, Serialize)]
pub struct InventorySnapshot {
    pub base_balance: f64,
    pub quote_balance: f64,
    pub mid_price: f64,
    pub normalized_inventory: f64,
    pub max_position: f64,
}

/// Execution fill event consumed by the strategy core.
#[derive(Clone, Debug)]
pub struct FillEvent {
    pub side: FillSide,
    pub price: f64,
    pub size: f64,
    pub timestamp: Instant,
}

#[derive(Clone, Copy, Debug)]
pub enum FillSide {
    Bid,
    Ask,
}

/// High-level events emitted by the strategy for logging/telemetry.
#[derive(Clone, Debug)]
pub enum StrategyEvent {
    Tick(MarketTick),
    Quote(QuotePair),
    Fill(FillEvent),
    CancelAll(&'static str),
    Warning(String),
    Error(String),
}

/// Aggregated runtime metrics for monitoring.
#[derive(Clone, Debug, Default, Serialize)]
pub struct StrategyMetrics {
    pub total_quotes: u64,
    pub total_fills: u64,
    pub last_quote_latency_ms: Option<f64>,
    pub sigma_annualized: Option<f64>,
    pub total_buy_volume: f64,
    pub total_sell_volume: f64,
    pub total_buy_cost: f64,
    pub total_sell_revenue: f64,
    pub net_pnl: f64,
    pub avg_buy_price: f64,
    pub avg_sell_price: f64,
    pub last_reservation_price: Option<f64>,
    pub last_raw_spread_bps: Option<f64>,
    pub last_effective_spread_bps: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct SafetyBounds {
    pub min_spread_bps: f64,
    pub max_spread_bps: f64,
    pub max_position: f64,
    pub min_notional: f64,
    pub volatility_breaker: f64,
    pub maker_fee_bps: f64,
    pub min_edge_bps_total: f64,
}

#[derive(Clone, Debug)]
pub enum StrategyDecision {
    Skip(&'static str),
    Cancel(&'static str),
    Quote(QuotePair),
    QuoteBidOnly(QuoteOrder),
    QuoteAskOnly(QuoteOrder),
}
