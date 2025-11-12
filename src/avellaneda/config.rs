use super::types::StrategyParams;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{env, fs, path::Path, time::Instant};

#[derive(Debug, Clone, Deserialize)]
pub struct AvellanedaConfig {
    pub market_id: i32,
    #[serde(default = "default_true")]
    pub dry_run: bool,
    #[serde(default = "default_false")]
    pub fast_execution: bool,
    #[serde(default = "default_false")]
    pub optimistic_batch_ack: bool,
    pub order_size: f64,
    pub gamma: f64,
    pub kappa: f64,
    pub time_horizon_hours: f64,
    pub target_base_pct: f64,
    pub vol_lookback: usize,
    pub vol_ewma_alpha: f64,
    pub refresh_interval_ms: u64,
    pub min_spread_bps: f64,
    pub max_spread_bps: f64,
    pub max_position: f64,
    pub min_notional: f64,
    #[serde(default = "default_vol_breaker")]
    pub volatility_breaker: f64,
    #[serde(default = "default_maker_fee")]
    pub maker_fee_bps: f64,
    #[serde(default = "default_min_edge_total")]
    pub min_edge_bps_total: f64,
    #[serde(default = "default_partial_fraction")]
    pub inventory_partial_fraction: f64,
    #[serde(default = "default_partial_age")]
    pub inventory_partial_age_secs: u64,
    #[serde(default = "default_partial_bps")]
    pub inventory_partial_bps: f64,
    #[serde(default = "default_partial_cooldown_ms")]
    pub inventory_partial_cooldown_ms: u64,
    #[serde(default = "default_flip_threshold")]
    pub flip_gate_threshold: usize,
    #[serde(default = "default_flip_pause_ms")]
    pub flip_pause_ms: u64,
    #[serde(default = "default_flip_widen_bps")]
    pub flip_widen_bps: f64,
    #[serde(default = "default_flip_widen_duration_ms")]
    pub flip_widen_duration_ms: u64,
    #[serde(default = "default_ladder_ratios")]
    pub ladder_ratios: Vec<f64>,
    #[serde(default = "default_ladder_offsets")]
    pub ladder_offsets_bps: Vec<f64>,
    #[serde(default = "default_ladder_sizes")]
    pub ladder_size_multipliers: Vec<f64>,
    #[serde(default = "default_ladder_hysteresis_ratio")]
    pub ladder_hysteresis_ratio: f64,
    #[serde(default = "default_kill_markout_bps")]
    pub kill_switch_markout_bps: f64,
    #[serde(default = "default_kill_pause_secs")]
    pub kill_pause_secs: u64,
    #[serde(default = "default_global_pnl_threshold")]
    pub global_pnl_threshold_bps_per_mm: f64,
    #[serde(default = "default_global_pause_secs")]
    pub global_pause_secs: u64,
    #[serde(default = "default_refresh_tolerance")]
    pub refresh_tolerance_ticks: i64,
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_vol_breaker() -> f64 {
    0.60
}

fn default_maker_fee() -> f64 {
    0.2
}

fn default_min_edge_total() -> f64 {
    1.1
}

fn default_partial_fraction() -> f64 {
    0.33
}

fn default_partial_age() -> u64 {
    8
}

fn default_partial_bps() -> f64 {
    6.0
}

fn default_partial_cooldown_ms() -> u64 {
    2000
}

fn default_flip_threshold() -> usize {
    8
}

fn default_flip_pause_ms() -> u64 {
    600
}

fn default_flip_widen_bps() -> f64 {
    1.0
}

fn default_flip_widen_duration_ms() -> u64 {
    1200
}

fn default_ladder_ratios() -> Vec<f64> {
    vec![0.25, 0.50, 0.75, 1.0]
}

fn default_ladder_offsets() -> Vec<f64> {
    vec![1.0, 3.0, 5.0, 8.0]
}

fn default_ladder_sizes() -> Vec<f64> {
    vec![1.0, 0.5, 0.25, 0.1]
}

fn default_ladder_hysteresis_ratio() -> f64 {
    0.05
}

fn default_kill_markout_bps() -> f64 {
    -5.0
}

fn default_kill_pause_secs() -> u64 {
    5
}

fn default_global_pnl_threshold() -> f64 {
    -50.0
}

fn default_global_pause_secs() -> u64 {
    10
}

fn default_refresh_tolerance() -> i64 {
    2
}

impl AvellanedaConfig {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let data =
            fs::read_to_string(path.as_ref()).with_context(|| "Failed to read config.toml")?;
        let mut raw: toml::Value =
            toml::from_str(&data).with_context(|| "Failed to parse TOML config")?;
        // Support nested [avellaneda] tables or top-level entries.
        let table = if let Some(table) = raw
            .get_mut("avellaneda")
            .and_then(|v| v.as_table_mut())
            .cloned()
        {
            table
        } else {
            raw.try_into()
                .map_err(|_| anyhow::anyhow!("Invalid avellaneda config structure"))?
        };
        let mut cfg: AvellanedaConfig = toml::from_str(&toml::to_string(&table)?)?;
        cfg.apply_env_overrides();
        cfg.validate()?;
        Ok(cfg)
    }

    fn apply_env_overrides(&mut self) {
        // Each field can be overridden by `AVELLANEDA_*` env vars.
        override_f64("AVELLANEDA_ORDER_SIZE", &mut self.order_size);
        override_f64("AVELLANEDA_GAMMA", &mut self.gamma);
        override_f64("AVELLANEDA_KAPPA", &mut self.kappa);
        override_f64(
            "AVELLANEDA_TIME_HORIZON_HOURS",
            &mut self.time_horizon_hours,
        );
        override_f64("AVELLANEDA_TARGET_BASE_PCT", &mut self.target_base_pct);
        override_usize("AVELLANEDA_VOL_LOOKBACK", &mut self.vol_lookback);
        override_f64("AVELLANEDA_VOL_EWMA_ALPHA", &mut self.vol_ewma_alpha);
        override_u64(
            "AVELLANEDA_REFRESH_INTERVAL_MS",
            &mut self.refresh_interval_ms,
        );
        override_f64("AVELLANEDA_MIN_SPREAD_BPS", &mut self.min_spread_bps);
        override_f64("AVELLANEDA_MAX_SPREAD_BPS", &mut self.max_spread_bps);
        override_f64("AVELLANEDA_MAX_POSITION", &mut self.max_position);
        override_f64("AVELLANEDA_MIN_NOTIONAL", &mut self.min_notional);
        override_bool("AVELLANEDA_DRY_RUN", &mut self.dry_run);
        override_bool("AVELLANEDA_FAST_EXECUTION", &mut self.fast_execution);
        override_bool("AVELLANEDA_OPTIMISTIC_ACK", &mut self.optimistic_batch_ack);
        override_f64(
            "AVELLANEDA_VOLATILITY_BREAKER",
            &mut self.volatility_breaker,
        );
        override_f64("AVELLANEDA_MAKER_FEE_BPS", &mut self.maker_fee_bps);
        override_f64(
            "AVELLANEDA_MIN_EDGE_BPS_TOTAL",
            &mut self.min_edge_bps_total,
        );
        override_f64(
            "AVELLANEDA_INVENTORY_PARTIAL_FRACTION",
            &mut self.inventory_partial_fraction,
        );
        override_u64(
            "AVELLANEDA_INVENTORY_PARTIAL_AGE_SECS",
            &mut self.inventory_partial_age_secs,
        );
        override_f64(
            "AVELLANEDA_INVENTORY_PARTIAL_BPS",
            &mut self.inventory_partial_bps,
        );
        override_u64(
            "AVELLANEDA_INVENTORY_PARTIAL_COOLDOWN_MS",
            &mut self.inventory_partial_cooldown_ms,
        );
        override_usize(
            "AVELLANEDA_FLIP_GATE_THRESHOLD",
            &mut self.flip_gate_threshold,
        );
        override_u64("AVELLANEDA_FLIP_PAUSE_MS", &mut self.flip_pause_ms);
        override_f64("AVELLANEDA_FLIP_WIDEN_BPS", &mut self.flip_widen_bps);
        override_u64(
            "AVELLANEDA_FLIP_WIDEN_DURATION_MS",
            &mut self.flip_widen_duration_ms,
        );
        override_f64(
            "AVELLANEDA_KILL_MARKOUT_BPS",
            &mut self.kill_switch_markout_bps,
        );
        override_u64("AVELLANEDA_KILL_PAUSE_SECS", &mut self.kill_pause_secs);
        override_f64(
            "AVELLANEDA_GLOBAL_PNL_THRESHOLD_BPS_PER_MM",
            &mut self.global_pnl_threshold_bps_per_mm,
        );
        override_u64("AVELLANEDA_GLOBAL_PAUSE_SECS", &mut self.global_pause_secs);
        if let Ok(value) = env::var("AVELLANEDA_REFRESH_TOLERANCE_TICKS") {
            if let Ok(parsed) = value.parse::<i64>() {
                self.refresh_tolerance_ticks = parsed.max(0);
            }
        }
    }

    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            (0.01..=1.0).contains(&self.gamma),
            "gamma must be between 0.01 and 1.0"
        );
        anyhow::ensure!(
            (0.2..=5.0).contains(&self.kappa),
            "kappa must be between 0.2 and 5.0"
        );
        anyhow::ensure!(
            (0.1..=8.0).contains(&self.time_horizon_hours),
            "time_horizon_hours must be between 0.1 and 8.0"
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.target_base_pct),
            "target_base_pct must be within [0, 1]"
        );
        anyhow::ensure!(
            (10..=1000).contains(&self.vol_lookback),
            "vol_lookback must be between 10 and 1000"
        );
        anyhow::ensure!(
            (0.01..=0.5).contains(&self.vol_ewma_alpha),
            "vol_ewma_alpha must be between 0.01 and 0.5"
        );
        anyhow::ensure!(
            self.max_spread_bps >= self.min_spread_bps,
            "max_spread_bps must be >= min_spread_bps"
        );
        anyhow::ensure!(
            self.max_position > 0.0,
            "max_position must be greater than zero"
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.inventory_partial_fraction),
            "inventory_partial_fraction must be within [0, 1]"
        );
        anyhow::ensure!(
            self.min_edge_bps_total >= 0.0,
            "min_edge_bps_total must be non-negative"
        );
        let ladder_len = self.ladder_ratios.len();
        anyhow::ensure!(
            ladder_len == self.ladder_offsets_bps.len()
                && ladder_len == self.ladder_size_multipliers.len(),
            "ladder configuration lengths must match"
        );
        anyhow::ensure!(
            self.ladder_hysteresis_ratio >= 0.0 && self.ladder_hysteresis_ratio <= 0.5,
            "ladder_hysteresis_ratio must be within [0, 0.5]"
        );
        Ok(())
    }

    pub fn core_params(&self) -> StrategyParams {
        StrategyParams {
            gamma: self.gamma,
            kappa: self.kappa,
            order_size: self.order_size,
            time_horizon_hours: self.time_horizon_hours,
            start_time: Instant::now(),
        }
    }
}

fn override_f64(key: &str, field: &mut f64) {
    if let Ok(value) = env::var(key) {
        if let Ok(parsed) = value.parse::<f64>() {
            *field = parsed;
        }
    }
}

fn override_usize(key: &str, field: &mut usize) {
    if let Ok(value) = env::var(key) {
        if let Ok(parsed) = value.parse::<usize>() {
            *field = parsed;
        }
    }
}

fn override_u64(key: &str, field: &mut u64) {
    if let Ok(value) = env::var(key) {
        if let Ok(parsed) = value.parse::<u64>() {
            *field = parsed;
        }
    }
}

fn override_bool(key: &str, field: &mut bool) {
    if let Ok(value) = env::var(key) {
        if let Ok(parsed) = value.parse::<bool>() {
            *field = parsed;
        }
    }
}
