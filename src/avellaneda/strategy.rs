use super::{
    config::AvellanedaConfig,
    inventory::InventoryState,
    market_data::MarketDataState,
    participation::{
        LadderLevel, ParticipationConfig, ParticipationController, ParticipationMetricsSnapshot,
    },
    spreads,
    types::{
        FillEvent, FillSide, InventorySnapshot, QuoteContext, QuotePair, SafetyBounds,
        StrategyDecision, StrategyMetrics, StrategyParams,
    },
    volatility::VolEstimator,
};
use crate::ws_client::OrderBookState;
use std::{
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const INVENTORY_LIMIT_TOLERANCE: f64 = 0.95;

pub struct AvellanedaStrategy {
    pub config: AvellanedaConfig,
    params: StrategyParams,
    bounds: SafetyBounds,
    inventory: InventoryState,
    volatility: VolEstimator,
    market_data: MarketDataState,
    metrics: StrategyMetrics,
    samples_per_second: f64,
    warmup_logged: bool,
    participation: ParticipationController,
    last_quote_context: Option<QuoteContext>,
}

impl AvellanedaStrategy {
    pub fn new(config: AvellanedaConfig) -> Self {
        let params = config.core_params();
        let bounds = SafetyBounds {
            min_spread_bps: config.min_spread_bps,
            max_spread_bps: config.max_spread_bps,
            max_position: config.max_position,
            min_notional: config.min_notional,
            volatility_breaker: config.volatility_breaker,
            maker_fee_bps: config.maker_fee_bps,
            min_edge_bps_total: config.min_edge_bps_total,
        };
        let target_pct = config.target_base_pct;
        let max_pos = config.max_position;
        let vol_lookback = config.vol_lookback;
        let vol_alpha = config.vol_ewma_alpha;
        let refresh_ms = config.refresh_interval_ms;

        let inventory = InventoryState::new(target_pct, max_pos);
        let initial_snapshot = inventory.snapshot();
        let participation_config = build_participation_config(&config);
        let participation = ParticipationController::new(participation_config, initial_snapshot);

        Self {
            config,
            params,
            bounds,
            inventory,
            volatility: VolEstimator::new(vol_lookback, vol_alpha),
            market_data: MarketDataState::new(0.2),
            metrics: StrategyMetrics::default(),
            samples_per_second: if refresh_ms == 0 {
                1.0
            } else {
                1000.0 / (refresh_ms as f64)
            },
            warmup_logged: false,
            participation,
            last_quote_context: None,
        }
    }

    pub fn update_balances(&mut self, base_balance: f64, quote_balance: f64) {
        let mid = self
            .market_data
            .last_tick()
            .map(|tick| tick.mid)
            .unwrap_or(self.inventory.mid_price);
        self.inventory
            .update_balances(base_balance, quote_balance, mid);
    }

    pub fn on_fill(&mut self, fill: &FillEvent) {
        let inv_before = self.inventory.base_balance;
        let mid = self
            .market_data
            .last_tick()
            .map(|tick| tick.mid)
            .unwrap_or(self.inventory.mid_price);
        self.inventory.apply_fill(fill);
        self.metrics.total_fills += 1;

        match fill.side {
            FillSide::Bid => {
                self.metrics.total_buy_volume += fill.size;
                self.metrics.total_buy_cost += fill.price * fill.size;
                if self.metrics.total_buy_volume > 0.0 {
                    self.metrics.avg_buy_price =
                        self.metrics.total_buy_cost / self.metrics.total_buy_volume;
                }
            }
            FillSide::Ask => {
                self.metrics.total_sell_volume += fill.size;
                self.metrics.total_sell_revenue += fill.price * fill.size;
                if self.metrics.total_sell_volume > 0.0 {
                    self.metrics.avg_sell_price =
                        self.metrics.total_sell_revenue / self.metrics.total_sell_volume;
                }
            }
        }

        self.metrics.net_pnl = self.metrics.total_sell_revenue - self.metrics.total_buy_cost;
        let inv_after = self.inventory.base_balance;
        self.participation
            .on_fill(fill, mid, inv_before, inv_after, &self.metrics);
    }

    pub fn inventory_snapshot(&self) -> InventorySnapshot {
        self.inventory.snapshot()
    }

    pub fn on_order_book(&mut self, book: &OrderBookState, now: Instant) -> StrategyDecision {
        let Some(tick) = self.market_data.on_order_book(book, now) else {
            return StrategyDecision::Skip("no_top_of_book");
        };

        self.inventory.update_balances(
            self.inventory.base_balance,
            self.inventory.quote_balance,
            tick.mid,
        );
        let snapshot = self.inventory.snapshot();
        self.participation.update_inventory_snapshot(&snapshot, now);
        self.participation.on_market_tick(tick.mid, now);
        self.volatility.on_mid_price(tick.mid, tick.timestamp);

        let sigma_per_second = self.volatility.sigma_per_second(self.samples_per_second);
        let sigma_annualized = self.volatility.sigma_annualized(self.samples_per_second);
        self.metrics.sigma_annualized = Some(sigma_annualized);

        // Log when warmup completes (once)
        if !self.warmup_logged && self.volatility.is_warmed_up() {
            tracing::info!(
                "Volatility estimator warmed up | σ_annual={:.4} | breaker={:.4}",
                sigma_annualized,
                self.bounds.volatility_breaker
            );
            self.warmup_logged = true;
        }

        // Only apply volatility breaker after warmup period (avoids cold-start noise)
        if self.volatility.is_warmed_up() && sigma_annualized > self.bounds.volatility_breaker {
            return StrategyDecision::Cancel("volatility_breaker");
        }

        if self.params.order_size * tick.mid < self.bounds.min_notional {
            return StrategyDecision::Skip("below_min_notional");
        }

        let q = self.inventory.normalized_inventory();
        let quote_calc =
            match spreads::compute_quote(tick.mid, q, sigma_per_second, &self.params, &self.bounds)
            {
                Some(calc) => calc,
                None => return StrategyDecision::Skip("spread_guard"),
            };

        self.metrics.last_reservation_price = Some(quote_calc.context.reservation_price);
        self.metrics.last_raw_spread_bps = Some(quote_calc.context.raw_spread_bps);
        self.metrics.last_effective_spread_bps = Some(quote_calc.context.effective_spread_bps);
        self.last_quote_context = Some(quote_calc.context.clone());

        let mut quotes = quote_calc.pair;

        if self.inventory.close_to_limit(INVENTORY_LIMIT_TOLERANCE) {
            if self.inventory.base_balance > 0.0 {
                quotes.bid.size = 0.0;
                quotes.ask.label = "inventory_reduce_ask";
            } else if self.inventory.base_balance < 0.0 {
                quotes.ask.size = 0.0;
                quotes.bid.label = "inventory_reduce_bid";
            } else {
                return StrategyDecision::Cancel("inventory_limit");
            }
        }

        match self.participation.apply_participation_rules(quotes, now) {
            Some(decision) => decision,
            None => StrategyDecision::Skip("participation_filtered"),
        }
    }

    pub fn record_quote(&mut self) {
        self.metrics.total_quotes += 1;
    }

    pub fn participation_metrics(&self) -> ParticipationMetricsSnapshot {
        self.participation.metrics_snapshot()
    }

    pub fn metrics(&self) -> &StrategyMetrics {
        &self.metrics
    }

    pub fn params(&self) -> &StrategyParams {
        &self.params
    }

    pub fn params_mut(&mut self) -> &mut StrategyParams {
        &mut self.params
    }

    pub fn bounds(&self) -> &SafetyBounds {
        &self.bounds
    }
}

fn build_participation_config(cfg: &AvellanedaConfig) -> ParticipationConfig {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = PathBuf::from(format!("logs/fills/session-{}.csv", timestamp));
    let mut ladder = Vec::new();
    let len = cfg
        .ladder_ratios
        .len()
        .min(cfg.ladder_offsets_bps.len())
        .min(cfg.ladder_size_multipliers.len());
    for idx in 0..len {
        ladder.push(LadderLevel {
            ratio: cfg.ladder_ratios[idx],
            offset_bps: cfg.ladder_offsets_bps[idx],
            size_multiplier: cfg.ladder_size_multipliers[idx],
        });
    }
    if ladder.is_empty() {
        ladder = vec![
            LadderLevel {
                ratio: 0.25,
                offset_bps: 1.0,
                size_multiplier: 1.0,
            },
            LadderLevel {
                ratio: 0.50,
                offset_bps: 3.0,
                size_multiplier: 0.5,
            },
            LadderLevel {
                ratio: 0.75,
                offset_bps: 5.0,
                size_multiplier: 0.25,
            },
            LadderLevel {
                ratio: 1.00,
                offset_bps: 8.0,
                size_multiplier: 0.1,
            },
        ];
    }
    ParticipationConfig {
        enable_logging: true,
        fill_log_path: Some(path),
        base_pause_ms: 350,
        toxicity_pause_ms: 900,
        same_side_limit_per_sec: 3,
        kill_switch_markout_bps: cfg.kill_switch_markout_bps,
        kill_pause_secs: cfg.kill_pause_secs,
        global_pnl_threshold_bps_per_mm: cfg.global_pnl_threshold_bps_per_mm,
        global_pause_secs: cfg.global_pause_secs,
        flip_gate_threshold: cfg.flip_gate_threshold,
        flip_pause_ms: cfg.flip_pause_ms,
        flip_widen_bps: cfg.flip_widen_bps,
        flip_widen_duration_ms: cfg.flip_widen_duration_ms,
        inventory_age_threshold_s: cfg.inventory_partial_age_secs,
        inventory_partial_bps: cfg.inventory_partial_bps,
        inventory_partial_fraction: cfg.inventory_partial_fraction,
        inventory_partial_cooldown_ms: cfg.inventory_partial_cooldown_ms,
        ladder_hysteresis_ratio: cfg.ladder_hysteresis_ratio,
        base_order_size: cfg.order_size,
        ladder,
    }
}

impl Drop for AvellanedaStrategy {
    fn drop(&mut self) {
        self.participation.finalize();
    }
}
