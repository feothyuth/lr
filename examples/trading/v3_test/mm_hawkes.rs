//! Hawkes-enhanced two-sided market maker demo
//!
//! Splits transport across four WebSocket connections (order book, market stats,
//! account orders, transaction submissions) and drives prices/sizes with a simple
//! Hawkes toxicity model. This is a stepping stone toward the full
//! `gpt_mm_hawkes_patched.md` spec: it keeps the crash/guard infrastructure from
//! the simple bot, adds per-side intensity tracking, and adjusts spreads/sizes
//! accordingly while still issuing a cancel-all + recreate batch each refresh.

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    signer_client::SignerClient,
    tx_executor::{send_batch_tx_ws, TX_TYPE_CANCEL_ALL_ORDERS, TX_TYPE_CREATE_ORDER},
    types::{AccountId, ApiKeyIndex, BaseQty, MarketId, Nonce, Price},
    ws_client::{AccountEventEnvelope, OrderBookLevel, WsConnection, WsEvent, WsStream},
};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};
use tokio::time::{interval, sleep, MissedTickBehavior};

// --- Constants -----------------------------------------------------------------------

const MARKET_ID: i32 = 1;
const BASE_SPREAD_PCT: f64 = 0.00001; // 1 bp floor
const MAKER_FEE_PCT: f64 = 0.00002;   // 2 bps fees
const TARGET_EDGE_BPS: f64 = 0.35;    // ~3.5 bps target edge
const MARK_FRESH_FOR: Duration = Duration::from_millis(200);
const MIN_REFRESH_INTERVAL: Duration = Duration::from_millis(40);
const MAX_NO_MESSAGE_TIME: Duration = Duration::from_secs(15);
const ACCOUNT_IDLE_GRACE: Duration = Duration::from_secs(60);

const WATCHDOG_CROSSED_FRAMES: u32 = 5;
const WATCHDOG_UNCHANGED_FRAMES: u32 = 4;
const WATCHDOG_CROSSED_TIME: Duration = Duration::from_millis(120);
const WATCHDOG_UNCHANGED_TIME: Duration = Duration::from_millis(150);
const WATCHDOG_STALE_TIME: Duration = Duration::from_millis(200);
const WATCHDOG_BASE_COOLDOWN: Duration = Duration::from_millis(250);
const WATCHDOG_MAX_COOLDOWN: Duration = Duration::from_millis(2000);
const WATCHDOG_RESUB_DELAY_BASE: Duration = Duration::from_millis(30);
const WATCHDOG_RESUB_JITTER_STEP: Duration = Duration::from_millis(10);
const WATCHDOG_RESUB_WINDOW: Duration = Duration::from_secs(60);
const WATCHDOG_ESCALATE_THRESHOLD: usize = 3;
const WATCHDOG_CLEAN_FRAMES: u32 = 2;
const WATCHDOG_CROSS_WINDOW: Duration = Duration::from_secs(10);
const WATCHDOG_JITTER_WINDOW: Duration = Duration::from_secs(5);
const GUARD_MIN_TTL: Duration = Duration::from_millis(200);
const GUARD_MAX_TTL: Duration = Duration::from_millis(1500);
const GUARD_DEFAULT_TTL: Duration = Duration::from_millis(600);
const GUARD_TTL_MULTIPLIER: f64 = 4.0;
const GUARD_EMA_ALPHA: f64 = 0.2;
const GUARD_MAX_SCAN_LEVELS: usize = 8;
const GUARD_MAX_CONSEC_CROSSED: u32 = 6;
const GUARD_MIN_UPDATES: u32 = 8;
const PRICE_EPS: f64 = 1e-9;

const REST_REFRESH_ENABLED: bool = true;
const REST_REFRESH_COOLDOWN: Duration = Duration::from_secs(3);
const REST_SNAPSHOT_DEPTH: i64 = 100;

const DRY_RUN_ENV: &str = "MM_HAWKES_DRY_RUN";
const SIMULATE_FILLS_ENV: &str = "MM_HAWKES_SIMULATE_FILLS";

// Hawkes defaults (tune per market in production)
const HAWKES_MU_BID: f64 = 2.0;
const HAWKES_MU_ASK: f64 = 2.0;
const HAWKES_ALPHA_BID: f64 = 4.0;
const HAWKES_ALPHA_ASK: f64 = 4.0;
const HAWKES_BETA_BID: f64 = 6.0;
const HAWKES_BETA_ASK: f64 = 6.0;

// λ⊙ spread mapping defaults
const LAMBDA_STAR_BID: f64 = 3.0;
const LAMBDA_STAR_ASK: f64 = 3.0;
const K_BID: f64 = 6.0;
const K_ASK: f64 = 6.0;
const MIN_TICKS: f64 = 1.0;
const MAX_TICKS: f64 = 100.0;  // Allow wider spreads for inventory management
const SIZE_DAMP_C: f64 = 0.15;
const Q_MAX: f64 = 0.0004; // 0.4 clip baseline (2x min for dampening headroom)
const Q_MIN_EXCHANGE: f64 = 0.0002; // exchange-enforced minimum order size

const SOFT_POS_CAP: f64 = 0.010; // ~1 clip (in base units) - balanced threshold
const HARD_POS_CAP: f64 = 0.02;  // stop quoting heavy side at ~2 clips
const PENDING_ORDER_TTL: Duration = Duration::from_millis(120);
const MAX_INV_SKEW_BPS: f64 = 8.0; // quadratic inventory skew ceiling - increased for meaningful widening
const LADDER_THRESHOLD: f64 = 0.004; // trigger exit ladder when inventory ≥ 2 clips
const CRASH_R60_THRESHOLD: f64 = -0.01;
const CRASH_R10_THRESHOLD: f64 = -0.005;
const CRASH_VOL_MULTIPLIER: f64 = 3.0;
const CRASH_DEBOUNCE: Duration = Duration::from_millis(150);
const CRASH_COOLDOWN: Duration = Duration::from_secs(75);
const CRASH_HALF_SPREAD_BPS: f64 = 5.5;
const CRASH_SIZE_FACTOR: f64 = 0.35;
const CRASH_TRIM_RATIO: f64 = 0.4;
const CRASH_TRIM_MIN_QTY: f64 = 0.0002;
const MARKOUT_MEDIAN_WINDOW: usize = 50;
const TELEMETRY_REPORT_INTERVAL: Duration = Duration::from_secs(10);
const VOL_EWMA_ALPHA: f64 = 0.129445; // 10s half-life over 2s bars
const VOL_SPREAD_MULTIPLIER: f64 = 0.3;
const VOL_BAR_DURATION: Duration = Duration::from_secs(2);
const OFI_DECAY_ALPHA: f64 = 0.95;
const OFI_DEFAULT_WEIGHT: f64 = 0.0;
const QUEUE_THRESHOLD_MULTIPLIER: f64 = 6.0;

// --- Utility structs -----------------------------------------------------------------

#[derive(Clone, Copy)]
enum ResubReason {
    Crossed,
    Unchanged,
    Stale,
}

impl ResubReason {
    fn as_str(&self) -> &'static str {
        match self {
            ResubReason::Crossed => "crossed",
            ResubReason::Unchanged => "unchanged",
            ResubReason::Stale => "stale",
        }
    }
}

enum WatchdogDecision {
    None,
    Reconnect {
        delay: Duration,
        reason: ResubReason,
        attempts: usize,
        escalate: bool,
    },
}

#[derive(Clone, Copy, Debug)]
enum QuoteSide {
    Bid,
    Ask,
}

struct HawkesSide {
    mu: f64,
    alpha: f64,
    beta: f64,
    lambda: f64,
    last_ts: f64,
}

impl HawkesSide {
    fn new(mu: f64, alpha: f64, beta: f64, now: f64) -> Self {
        Self {
            mu,
            alpha,
            beta,
            lambda: mu,
            last_ts: now,
        }
    }

    fn decay_to(&mut self, now: f64) {
        if now <= self.last_ts {
            return;
        }
        let dt = now - self.last_ts;
        let decay = (-self.beta * dt).exp();
        self.lambda = self.mu + (self.lambda - self.mu) * decay;
        self.last_ts = now;
    }

    fn on_event(&mut self, now: f64) {
        self.decay_to(now);
        self.lambda += self.alpha;
    }

    fn value(&mut self, now: f64) -> f64 {
        self.decay_to(now);
        self.lambda.max(1e-6)
    }
}

struct QuoteMapParams {
    k_bid: f64,
    k_ask: f64,
    lam_star_bid: f64,
    lam_star_ask: f64,
    min_ticks: f64,
    max_ticks: f64,
    c_size: f64,
    q_max: f64,
    min_clip: f64,
}

struct QuoteDecision {
    bid_price: f64,
    ask_price: f64,
    bid_qty: f64,
    ask_qty: f64,
    lambda_bid: f64,
    lambda_ask: f64,
}

struct OrderTrackerEntry {
    remaining: f64,
    side: QuoteSide,
    price_ticks: i64,
}

#[allow(dead_code)]
struct SideOrderState {
    order_index: i64,
    price_ticks: i64,
    base_qty_units: i64,
}

#[derive(Default)]
struct ActiveOrders {
    bid: Option<SideOrderState>,
    ask: Option<SideOrderState>,
}

enum CrashState {
    Normal,
    CrashActive { since: Instant },
}

enum CrashSignal {
    None,
    Enter,
    Maintain,
    Exit,
}

struct PendingQuote {
    price_ticks: i64,
    base_qty_units: i64,
    last_update: Instant,
}

#[derive(Default)]
struct PendingOrders {
    bid: Option<PendingQuote>,
    ask: Option<PendingQuote>,
}

#[derive(Clone)]
struct DesiredQuote {
    price_ticks: i64,
    base_qty_units: i64,
}

struct MarkOutTracker {
    pending: VecDeque<(Instant, f64, QuoteSide)>,
    completed: VecDeque<f64>,
    median_window: usize,
    last_median: Option<f64>,
}

struct FillBreaker {
    bid_fills: VecDeque<Instant>,
    ask_fills: VecDeque<Instant>,
    bid_paused_until: Option<Instant>,
    ask_paused_until: Option<Instant>,
    bid_extra_ticks: u32,
    ask_extra_ticks: u32,
}

#[derive(Default)]
struct QuoteCache {
    last_bid: Option<(i64, i64)>,
    last_ask: Option<(i64, i64)>,
}

struct Telemetry {
    fills: VecDeque<(Instant, f64, QuoteSide)>,
    po_attempts: u64,
    po_rejects: u64,
    position_history: VecDeque<(Instant, f64)>,
    last_report: Instant,
}

struct VolatilityEstimator {
    ewma_vol: f64,
    alpha: f64,
    bar_duration: Duration,
    initialized: bool,
    current_start: Instant,
    current_high: f64,
    current_low: f64,
}

struct OfiTracker {
    enabled: bool,
    weight: f64,
    decay_alpha: f64,
    last_bid_size: Option<f64>,
    last_ask_size: Option<f64>,
    bid_adds: f64,
    bid_cancels: f64,
    ask_adds: f64,
    ask_cancels: f64,
}

struct CrashDetector {
    price_history: VecDeque<(Instant, f64)>,
    baseline_volatility: f64,
    current_volatility: f64,
    baseline_depth3: f64,
    tick_size: f64,
    pending_trigger: Option<Instant>,
    crash_state: CrashState,
}

impl PendingOrders {
    fn mark(&mut self, side: QuoteSide, price_ticks: i64, base_qty_units: i64, now: Instant) {
        let entry = PendingQuote {
            price_ticks,
            base_qty_units,
            last_update: now,
        };
        match side {
            QuoteSide::Bid => self.bid = Some(entry),
            QuoteSide::Ask => self.ask = Some(entry),
        }
    }

    fn clear(&mut self, side: QuoteSide) {
        match side {
            QuoteSide::Bid => self.bid = None,
            QuoteSide::Ask => self.ask = None,
        }
    }

    fn matches_recent(
        &self,
        side: QuoteSide,
        price_ticks: i64,
        base_qty_units: i64,
        now: Instant,
        ttl: Duration,
    ) -> bool {
        let entry = match side {
            QuoteSide::Bid => self.bid.as_ref(),
            QuoteSide::Ask => self.ask.as_ref(),
        };
        if let Some(pending) = entry {
            if now.duration_since(pending.last_update) <= ttl
                && pending.price_ticks == price_ticks
                && pending.base_qty_units == base_qty_units
            {
                return true;
            }
        }
        false
    }
}

fn create_exit_ladder(
    inventory: f64,
    mid: f64,
    tick_size: f64,
    size_multiplier: i64,
) -> Vec<(QuoteSide, DesiredQuote)> {
    let abs_inv = inventory.abs();
    if abs_inv < LADDER_THRESHOLD {
        return Vec::new();
    }

    let is_long = inventory > 0.0;
    let num_rungs = if abs_inv > 0.010 {
        4
    } else if abs_inv > 0.006 {
        3
    } else {
        2
    };

    let weights: &[f64] = match num_rungs {
        4 => &[0.4, 0.3, 0.2, 0.1],
        3 => &[0.5, 0.3, 0.2],
        _ => &[0.6, 0.4],
    };

    let mut quotes = Vec::new();
    for (idx, weight) in weights.iter().enumerate() {
        let offset = (idx + 1) as f64;
        let raw_price = if is_long {
            mid + offset * tick_size
        } else {
            (mid - offset * tick_size).max(tick_size)
        };
        let price_ticks = if is_long {
            (raw_price / tick_size).ceil() as i64
        } else {
            (raw_price / tick_size).floor() as i64
        };

        let rung_qty = (abs_inv * weight).max(Q_MIN_EXCHANGE);
        let units = qty_to_units(rung_qty, size_multiplier);
        if units <= 0 {
            continue;
        }

        quotes.push((
            if is_long { QuoteSide::Ask } else { QuoteSide::Bid },
            DesiredQuote {
                price_ticks,
                base_qty_units: units,
            },
        ));
    }

    quotes
}

impl MarkOutTracker {
    fn new(window: usize) -> Self {
        Self {
            pending: VecDeque::new(),
            completed: VecDeque::new(),
            median_window: window,
            last_median: None,
        }
    }

    fn on_fill(&mut self, now: Instant, fill_price: f64, side: QuoteSide) {
        self.pending
            .push_back((now + Duration::from_secs(1), fill_price, side));
    }

    fn update(&mut self, now: Instant, mid: f64) -> Option<f64> {
        while let Some((ready_time, fill_price, side)) = self.pending.front().copied() {
            if now >= ready_time {
                let markout = match side {
                    QuoteSide::Bid => mid - fill_price,
                    QuoteSide::Ask => fill_price - mid,
                };
                self.completed.push_back(markout);
                if self.completed.len() > self.median_window {
                    self.completed.pop_front();
                }
                self.pending.pop_front();
            } else {
                break;
            }
        }

        if self.completed.len() >= 10 {
            let mut sorted: Vec<f64> = self.completed.iter().copied().collect();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = sorted[sorted.len() / 2];
            self.last_median = Some(median);
        } else {
            self.last_median = None;
        }

        self.last_median
    }

    fn heavy_side_bump(&self, inventory: f64, tick_size: f64) -> (f64, f64) {
        if let Some(median) = self.last_median {
            if median < 0.0 {
                if inventory > 0.0 {
                    return (tick_size, 0.0);
                } else if inventory < 0.0 {
                    return (0.0, tick_size);
                }
            }
        }
        (0.0, 0.0)
    }

    fn median(&self) -> Option<f64> {
        self.last_median
    }

}

impl FillBreaker {
    fn new() -> Self {
        Self {
            bid_fills: VecDeque::new(),
            ask_fills: VecDeque::new(),
            bid_paused_until: None,
            ask_paused_until: None,
            bid_extra_ticks: 0,
            ask_extra_ticks: 0,
        }
    }

    fn on_fill(&mut self, side: QuoteSide, now: Instant) {
        let queue = match side {
            QuoteSide::Bid => &mut self.bid_fills,
            QuoteSide::Ask => &mut self.ask_fills,
        };
        queue.push_back(now);
        while let Some(ts) = queue.front() {
            if now.duration_since(*ts) > Duration::from_millis(300) {
                queue.pop_front();
            } else {
                break;
            }
        }

        if queue.len() >= 3 {
            match side {
                QuoteSide::Bid => {
                    self.bid_paused_until = Some(now + Duration::from_millis(400));
                    self.bid_extra_ticks = self.bid_extra_ticks.max(1);
                }
                QuoteSide::Ask => {
                    self.ask_paused_until = Some(now + Duration::from_millis(400));
                    self.ask_extra_ticks = self.ask_extra_ticks.max(1);
                }
            }
            queue.clear();
        }
    }

    fn should_quote(&self, side: QuoteSide, now: Instant) -> bool {
        match side {
            QuoteSide::Bid => self
                .bid_paused_until
                .map_or(true, |until| now >= until),
            QuoteSide::Ask => self
                .ask_paused_until
                .map_or(true, |until| now >= until),
        }
    }

    fn take_extra_ticks(&mut self, side: QuoteSide) -> u32 {
        match side {
            QuoteSide::Bid => {
                let extra = self.bid_extra_ticks;
                self.bid_extra_ticks = 0;
                extra
            }
            QuoteSide::Ask => {
                let extra = self.ask_extra_ticks;
                self.ask_extra_ticks = 0;
                extra
            }
        }
    }
}

impl QuoteCache {
    fn should_update_bid(&self, desired: &Option<DesiredQuote>) -> bool {
        Self::should_update(desired, self.last_bid)
    }

    fn should_update_ask(&self, desired: &Option<DesiredQuote>) -> bool {
        Self::should_update(desired, self.last_ask)
    }

    fn record_bid(&mut self, desired: &Option<DesiredQuote>) {
        self.last_bid = desired
            .as_ref()
            .map(|d| (d.price_ticks, d.base_qty_units));
    }

    fn record_ask(&mut self, desired: &Option<DesiredQuote>) {
        self.last_ask = desired
            .as_ref()
            .map(|d| (d.price_ticks, d.base_qty_units));
    }

    fn should_update(desired: &Option<DesiredQuote>, last: Option<(i64, i64)>) -> bool {
        match (desired, last) {
            (None, None) => false,
            (None, Some(_)) => true,
            (Some(_), None) => true,
            (Some(new), Some((price_ticks, qty_units))) => {
                (new.price_ticks - price_ticks).abs() >= 1
                    || Self::size_changed(new.base_qty_units, qty_units)
            }
        }
    }

    fn size_changed(new_units: i64, last_units: i64) -> bool {
        if last_units == 0 {
            return new_units != 0;
        }
        let diff = (new_units - last_units).abs() as f64;
        diff / last_units.abs() as f64 > 0.2
    }
}

impl Telemetry {
    fn new() -> Self {
        Self {
            fills: VecDeque::new(),
            po_attempts: 0,
            po_rejects: 0,
            position_history: VecDeque::new(),
            last_report: Instant::now(),
        }
    }

    fn on_fill(&mut self, now: Instant, size: f64, side: QuoteSide) {
        self.fills.push_back((now, size.abs(), side));
        while let Some((ts, _, _)) = self.fills.front() {
            if now.duration_since(*ts) > Duration::from_secs(60) {
                self.fills.pop_front();
            } else {
                break;
            }
        }
    }

    fn on_position(&mut self, now: Instant, inventory: f64) {
        self.position_history.push_back((now, inventory));
        while let Some((ts, _)) = self.position_history.front() {
            if now.duration_since(*ts) > Duration::from_secs(600) {
                self.position_history.pop_front();
            } else {
                break;
            }
        }
    }

    fn on_post_only_response(&mut self, accepted: bool) {
        self.po_attempts += 1;
        if !accepted {
            self.po_rejects += 1;
        }
    }

    fn report(
        &mut self,
        now: Instant,
        inventory: f64,
        lambda_bid: f64,
        lambda_ask: f64,
        markout_median: Option<f64>,
        ofi_bps: f64,
        vol_bps: f64,
    ) {
        if now.duration_since(self.last_report) < TELEMETRY_REPORT_INTERVAL {
            return;
        }

        let fills_per_sec = self.fills.len() as f64 / 60.0;
        let po_reject_rate = if self.po_attempts == 0 {
            0.0
        } else {
            self.po_rejects as f64 / self.po_attempts as f64
        };
        let lambda_imbalance = if lambda_bid > 0.0 && lambda_ask > 0.0 {
            (lambda_bid / lambda_ask).ln()
        } else {
            0.0
        };

        let time_to_flat = self.calculate_time_to_flat(now, inventory);

        eprintln!("\n📊 TELEMETRY");
        eprintln!("  Fills/sec (60s): {:.2}", fills_per_sec);
        eprintln!("  PO reject rate: {:.2}%", po_reject_rate * 100.0);
        eprintln!("  λ imbalance ln(λb/λa): {:.3}", lambda_imbalance);
        if let Some(ttf) = time_to_flat {
            eprintln!("  Time-to-flat: {:.1}s", ttf);
        }
        if let Some(median) = markout_median {
            eprintln!("  Mark-out median (1s): {:.6}", median);
        }
        eprintln!("  OFI adjust (bps): {:.3}", ofi_bps);
        eprintln!("  Volatility EWMA (bps): {:.3}", vol_bps);

        self.last_report = now;
    }

    fn calculate_time_to_flat(&self, now: Instant, inventory: f64) -> Option<f64> {
        if inventory.abs() < Q_MIN_EXCHANGE {
            return Some(0.0);
        }

        for &(ts, pos) in self.position_history.iter().rev() {
            if pos.signum() != inventory.signum() || pos.abs() < inventory.abs() {
                return Some(now.duration_since(ts).as_secs_f64());
            }
        }
        None
    }
}

impl VolatilityEstimator {
    fn new(alpha: f64, bar_duration: Duration) -> Self {
        Self {
            ewma_vol: 0.0,
            alpha,
            bar_duration,
            initialized: false,
            current_start: Instant::now(),
            current_high: 0.0,
            current_low: f64::MAX,
        }
    }

    fn update(&mut self, price: f64, now: Instant) {
        if !self.initialized {
            self.initialized = true;
            self.current_start = now;
            self.current_high = price;
            self.current_low = price;
            self.ewma_vol = 0.0;
            return;
        }

        self.current_high = self.current_high.max(price);
        self.current_low = self.current_low.min(price);

        if now.duration_since(self.current_start) >= self.bar_duration && self.current_low > 0.0 {
            let range = (self.current_high / self.current_low).ln();
            let range_vol = if range.is_finite() {
                (range.powi(2) / (4.0 * (2.0f64).ln())).sqrt()
            } else {
                0.0
            };

            if self.ewma_vol == 0.0 {
                self.ewma_vol = range_vol;
            } else {
                self.ewma_vol = self.alpha * range_vol + (1.0 - self.alpha) * self.ewma_vol;
            }

            self.current_start = now;
            self.current_high = price;
            self.current_low = price;
        }
    }

    fn volatility_bps(&self) -> f64 {
        (self.ewma_vol * 10000.0).max(0.0)
    }
}

impl OfiTracker {
    fn new(weight: f64, decay_alpha: f64) -> Self {
        Self {
            enabled: weight > 0.0,
            weight: weight.min(0.2).max(0.0),
            decay_alpha,
            last_bid_size: None,
            last_ask_size: None,
            bid_adds: 0.0,
            bid_cancels: 0.0,
            ask_adds: 0.0,
            ask_cancels: 0.0,
        }
    }

    fn on_book_update(&mut self, book: &lighter_client::ws_client::OrderBookState) {
        if !self.enabled {
            return;
        }

        let bid_size = book
            .bids
            .iter()
            .find(|lvl| level_active(lvl))
            .map(level_quantity)
            .unwrap_or(0.0);
        let ask_size = book
            .asks
            .iter()
            .find(|lvl| level_active(lvl))
            .map(level_quantity)
            .unwrap_or(0.0);

        if let Some(last) = self.last_bid_size {
            let delta = bid_size - last;
            if delta > 0.0 {
                self.bid_adds += delta;
            } else {
                self.bid_cancels += delta.abs();
            }
        }
        if let Some(last) = self.last_ask_size {
            let delta = ask_size - last;
            if delta > 0.0 {
                self.ask_adds += delta;
            } else {
                self.ask_cancels += delta.abs();
            }
        }

        self.last_bid_size = Some(bid_size);
        self.last_ask_size = Some(ask_size);

        self.bid_adds *= self.decay_alpha;
        self.bid_cancels *= self.decay_alpha;
        self.ask_adds *= self.decay_alpha;
        self.ask_cancels *= self.decay_alpha;
    }

    fn adjustment_bps(&self) -> f64 {
        if !self.enabled {
            return 0.0;
        }
        let bid_pressure = self.bid_adds - self.bid_cancels;
        let ask_pressure = self.ask_adds - self.ask_cancels;
        let numerator = bid_pressure - ask_pressure;
        let denom = bid_pressure.abs() + ask_pressure.abs() + 1.0;
        let imbalance = numerator / denom;
        imbalance * self.weight * 100.0
    }
}

impl CrashDetector {
    fn new(tick_size: f64) -> Self {
        Self {
            price_history: VecDeque::new(),
            baseline_volatility: 0.0,
            current_volatility: 0.0,
            baseline_depth3: 0.0,
            tick_size,
            pending_trigger: None,
            crash_state: CrashState::Normal,
        }
    }

    fn is_active(&self) -> bool {
        matches!(self.crash_state, CrashState::CrashActive { .. })
    }

    fn update(
        &mut self,
        now: Instant,
        price: f64,
        best_bid: f64,
        best_ask: f64,
        book: &lighter_client::ws_client::OrderBookState,
    ) -> CrashSignal {
        self.price_history.push_back((now, price));
        while let Some((ts, _)) = self.price_history.front() {
            if now.duration_since(*ts) > Duration::from_secs(60) {
                self.price_history.pop_front();
            } else {
                break;
            }
        }

        let r60 = self.rolling_return(now, Duration::from_secs(60));
        let r10 = self.rolling_return(now, Duration::from_secs(10));

        self.current_volatility = self.compute_volatility();
        let depth3 = self.compute_depth3(book);
        if self.baseline_depth3 == 0.0 && depth3 > 0.0 {
            self.baseline_depth3 = depth3;
        }

        let s1 = best_ask - best_bid;
        if !self.is_active() {
            self.update_baselines(depth3);
        }

        let speed_trigger = r60.map_or(false, |r| r <= CRASH_R60_THRESHOLD)
            || r10.map_or(false, |r| r <= CRASH_R10_THRESHOLD);
        let vol_trigger = self.baseline_volatility > 0.0
            && self.current_volatility >= CRASH_VOL_MULTIPLIER * self.baseline_volatility;
        let quality_trigger = s1 >= 4.0 * self.tick_size
            || (self.baseline_depth3 > 0.0 && depth3 <= 0.4 * self.baseline_depth3);

        let triggered = (speed_trigger || vol_trigger) && quality_trigger;

        match self.crash_state {
            CrashState::Normal => {
                if triggered {
                    if let Some(start) = self.pending_trigger {
                        if now.duration_since(start) >= CRASH_DEBOUNCE {
                            self.crash_state = CrashState::CrashActive { since: now };
                            self.pending_trigger = None;
                            return CrashSignal::Enter;
                        }
                    } else {
                        self.pending_trigger = Some(now);
                    }
                } else {
                    self.pending_trigger = None;
                }
                CrashSignal::None
            }
            CrashState::CrashActive { since } => {
                if now.duration_since(since) >= CRASH_COOLDOWN {
                    self.crash_state = CrashState::Normal;
                    self.pending_trigger = None;
                    CrashSignal::Exit
                } else {
                    CrashSignal::Maintain
                }
            }
        }
    }

    fn rolling_return(&self, now: Instant, window: Duration) -> Option<f64> {
        let current_price = self.price_history.back()?.1;
        for (ts, price) in self.price_history.iter().rev() {
            if now.duration_since(*ts) >= window {
                if *price > 0.0 {
                    return Some((current_price - price) / price);
                } else {
                    return None;
                }
            }
        }
        None
    }

    fn compute_volatility(&self) -> f64 {
        if self.price_history.len() < 2 {
            return 0.0;
        }
        let mut returns = Vec::new();
        let mut iter = self.price_history.iter();
        let mut prev = match iter.next() {
            Some((_, price)) => *price,
            None => return 0.0,
        };
        for (_, price) in iter {
            if prev > 0.0 && *price > 0.0 {
                returns.push((*price / prev).ln());
            }
            prev = *price;
        }
        if returns.len() < 2 {
            return 0.0;
        }
        let mean = returns.iter().copied().sum::<f64>() / returns.len() as f64;
        let variance = returns
            .iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>()
            / returns.len() as f64;
        variance.sqrt()
    }

    fn compute_depth3(&self, book: &lighter_client::ws_client::OrderBookState) -> f64 {
        let depth_bid: f64 = book
            .bids
            .iter()
            .filter(|lvl| level_active(lvl))
            .take(3)
            .map(level_quantity)
            .sum();
        let depth_ask: f64 = book
            .asks
            .iter()
            .filter(|lvl| level_active(lvl))
            .take(3)
            .map(level_quantity)
            .sum();
        depth_bid.min(depth_ask)
    }

    fn update_baselines(&mut self, depth3: f64) {
        if self.current_volatility > 0.0 {
            self.baseline_volatility = if self.baseline_volatility == 0.0 {
                self.current_volatility
            } else {
                0.9 * self.baseline_volatility + 0.1 * self.current_volatility
            };
        }

        if depth3 > 0.0 {
            self.baseline_depth3 = if self.baseline_depth3 == 0.0 {
                depth3
            } else {
                0.9 * self.baseline_depth3 + 0.1 * depth3
            };
        }
    }
}

// --- Watchdog ------------------------------------------------------------------------

struct MarketWatchdog {
    last_best_ticks: Option<(i64, i64)>,
    last_good: Option<(i64, i64, Instant)>,
    crossed_streak: u32,
    crossed_start: Option<Instant>,
    unchanged_streak: u32,
    unchanged_start: Option<Instant>,
    last_resub: Option<Instant>,
    resub_attempts: VecDeque<Instant>,
    current_cooldown: Duration,
    pending: bool,
    last_crosses: VecDeque<Instant>,
    jitter_mode_until: Option<Instant>,
    clean_streak: u32,
}

impl MarketWatchdog {
    fn new() -> Self {
        Self {
            last_best_ticks: None,
            last_good: None,
            crossed_streak: 0,
            crossed_start: None,
            unchanged_streak: 0,
            unchanged_start: None,
            last_resub: None,
            resub_attempts: VecDeque::new(),
            current_cooldown: WATCHDOG_BASE_COOLDOWN,
            pending: false,
            last_crosses: VecDeque::new(),
            jitter_mode_until: None,
            clean_streak: 0,
        }
    }

    fn observe(&mut self, bid_ticks: i64, ask_ticks: i64, now: Instant) -> WatchdogDecision {
        let crossed = ask_ticks <= bid_ticks;
        let unchanged = self
            .last_best_ticks
            .map(|(prev_bid, prev_ask)| prev_bid == bid_ticks && prev_ask == ask_ticks)
            .unwrap_or(false);

        if crossed {
            self.crossed_streak = self.crossed_streak.saturating_add(1);
            if self.crossed_start.is_none() {
                self.crossed_start = Some(now);
            }
            self.clean_streak = 0;
            while let Some(front) = self.last_crosses.front() {
                if now.duration_since(*front) > WATCHDOG_CROSS_WINDOW {
                    self.last_crosses.pop_front();
                } else {
                    break;
                }
            }
            self.last_crosses.push_back(now);
        } else {
            self.crossed_streak = 0;
            self.crossed_start = None;
            self.clean_streak = self.clean_streak.saturating_add(1);
        }

        if unchanged {
            self.unchanged_streak = self.unchanged_streak.saturating_add(1);
            if self.unchanged_start.is_none() {
                self.unchanged_start = Some(now);
            }
        } else {
            self.unchanged_streak = 0;
            self.unchanged_start = None;
        }

        self.last_best_ticks = Some((bid_ticks, ask_ticks));

        if !crossed {
            self.last_good = Some((bid_ticks, ask_ticks, now));
            if self.pending && self.clean_streak >= WATCHDOG_CLEAN_FRAMES {
                self.mark_recovered();
            }
            return WatchdogDecision::None;
        }

        let crossed_duration = self
            .crossed_start
            .map(|start| now - start)
            .unwrap_or_default();
        let unchanged_duration = self
            .unchanged_start
            .map(|start| now - start)
            .unwrap_or_default();
        let stale_duration = self
            .last_good
            .map(|(_, _, ts)| now - ts)
            .unwrap_or_default();

        let crossed_trigger = self.crossed_streak >= WATCHDOG_CROSSED_FRAMES
            || crossed_duration >= WATCHDOG_CROSSED_TIME;
        let unchanged_trigger = self.unchanged_streak >= WATCHDOG_UNCHANGED_FRAMES
            || unchanged_duration >= WATCHDOG_UNCHANGED_TIME;
        let stale_trigger = stale_duration >= WATCHDOG_STALE_TIME;

        if (crossed_trigger || unchanged_trigger || stale_trigger) && self.can_trigger(now) {
            self.pending = true;
            self.last_resub = Some(now);
            let fast = self.should_fast_reconnect(now);

            self.resub_attempts.push_back(now);
            while let Some(front) = self.resub_attempts.front() {
                if now.duration_since(*front) > WATCHDOG_RESUB_WINDOW {
                    self.resub_attempts.pop_front();
                } else {
                    break;
                }
            }

            let attempts = self.resub_attempts.len();
            let delay = if fast {
                self.current_cooldown = WATCHDOG_BASE_COOLDOWN;
                Duration::ZERO
            } else {
                self.current_cooldown = (self.current_cooldown * 2).min(WATCHDOG_MAX_COOLDOWN);
                let jitter_steps = (attempts % 3) as u32;
                WATCHDOG_RESUB_DELAY_BASE + WATCHDOG_RESUB_JITTER_STEP * (jitter_steps + 1)
            };
            let reason = if crossed_trigger {
                ResubReason::Crossed
            } else if unchanged_trigger {
                ResubReason::Unchanged
            } else {
                ResubReason::Stale
            };
            let escalate = attempts >= WATCHDOG_ESCALATE_THRESHOLD;

            return WatchdogDecision::Reconnect {
                delay,
                reason,
                attempts,
                escalate,
            };
        }

        WatchdogDecision::None
    }

    fn can_trigger(&self, now: Instant) -> bool {
        self.last_resub
            .map(|ts| now.duration_since(ts) >= self.current_cooldown)
            .unwrap_or(true)
    }

    fn should_fast_reconnect(&mut self, now: Instant) -> bool {
        if self.last_crosses.len() <= 1 || self.jitter_mode_until.map_or(true, |until| now >= until) {
            self.jitter_mode_until = None;
            true
        } else {
            self.jitter_mode_until = Some(now + WATCHDOG_JITTER_WINDOW);
            false
        }
    }

    fn mark_recovered(&mut self) {
        self.pending = false;
        self.crossed_streak = 0;
        self.crossed_start = None;
        self.unchanged_streak = 0;
        self.unchanged_start = None;
        self.current_cooldown = WATCHDOG_BASE_COOLDOWN;
        self.resub_attempts.clear();
        self.last_resub = None;
        self.clean_streak = 0;
        self.jitter_mode_until = None;
    }
}

enum GuardOutcome {
    Bbo { bid: f64, ask: f64, skipped: bool },
    Escalate { reason: &'static str },
}

struct TobGuardConfig {
    min_ttl: Duration,
    max_ttl: Duration,
    default_ttl: Duration,
    ttl_multiplier: f64,
    max_scan_levels: usize,
    max_cross_frames: u32,
    min_updates: u32,
}

impl Default for TobGuardConfig {
    fn default() -> Self {
        Self {
            min_ttl: GUARD_MIN_TTL,
            max_ttl: GUARD_MAX_TTL,
            default_ttl: GUARD_DEFAULT_TTL,
            ttl_multiplier: GUARD_TTL_MULTIPLIER,
            max_scan_levels: GUARD_MAX_SCAN_LEVELS,
            max_cross_frames: GUARD_MAX_CONSEC_CROSSED,
            min_updates: GUARD_MIN_UPDATES,
        }
    }
}

struct TobGuard {
    last_bid: Option<f64>,
    last_ask: Option<f64>,
    last_bid_changed_at: Option<Instant>,
    last_ask_changed_at: Option<Instant>,
    updates_since_bid_change: u32,
    updates_since_ask_change: u32,
    ema_bid_interval: Option<f64>,
    ema_ask_interval: Option<f64>,
    crossed_frames: u32,
}

impl TobGuard {
    fn new() -> Self {
        Self {
            last_bid: None,
            last_ask: None,
            last_bid_changed_at: None,
            last_ask_changed_at: None,
            updates_since_bid_change: 0,
            updates_since_ask_change: 0,
            ema_bid_interval: None,
            ema_ask_interval: None,
            crossed_frames: 0,
        }
    }

    fn on_frame_start(&mut self, now: Instant, best_bid: f64, best_ask: f64) {
        match self.last_bid {
            Some(prev) if (best_bid - prev).abs() <= PRICE_EPS => {
                self.updates_since_bid_change = self.updates_since_bid_change.saturating_add(1);
            }
            _ => {
                if let Some(ts) = self.last_bid_changed_at {
                    let interval = now.duration_since(ts).as_secs_f64();
                    self.ema_bid_interval = Some(match self.ema_bid_interval {
                        Some(ema) => ema * (1.0 - GUARD_EMA_ALPHA) + interval * GUARD_EMA_ALPHA,
                        None => interval,
                    });
                }
                self.last_bid = Some(best_bid);
                self.last_bid_changed_at = Some(now);
                self.updates_since_bid_change = 0;
            }
        }

        match self.last_ask {
            Some(prev) if (best_ask - prev).abs() <= PRICE_EPS => {
                self.updates_since_ask_change = self.updates_since_ask_change.saturating_add(1);
            }
            _ => {
                if let Some(ts) = self.last_ask_changed_at {
                    let interval = now.duration_since(ts).as_secs_f64();
                    self.ema_ask_interval = Some(match self.ema_ask_interval {
                        Some(ema) => ema * (1.0 - GUARD_EMA_ALPHA) + interval * GUARD_EMA_ALPHA,
                        None => interval,
                    });
                }
                self.last_ask = Some(best_ask);
                self.last_ask_changed_at = Some(now);
                self.updates_since_ask_change = 0;
            }
        }
    }

    fn ttl_from_ema(&self, ema_secs: Option<f64>, cfg: &TobGuardConfig) -> Duration {
        let ttl_secs = ema_secs
            .map(|ema| (ema * cfg.ttl_multiplier).clamp(cfg.min_ttl.as_secs_f64(), cfg.max_ttl.as_secs_f64()))
            .unwrap_or(cfg.default_ttl.as_secs_f64());
        Duration::from_secs_f64(ttl_secs)
    }

    fn ask_is_suspect(&self, now: Instant, best_bid: f64, best_ask: f64, cfg: &TobGuardConfig) -> bool {
        best_ask <= best_bid + PRICE_EPS
            && self
                .last_ask_changed_at
                .map(|ts| now.duration_since(ts) >= self.ttl_from_ema(self.ema_ask_interval, cfg))
                .unwrap_or(false)
            && self.updates_since_ask_change >= cfg.min_updates
    }

    fn bid_is_suspect(&self, now: Instant, best_bid: f64, best_ask: f64, cfg: &TobGuardConfig) -> bool {
        best_bid >= best_ask - PRICE_EPS
            && self
                .last_bid_changed_at
                .map(|ts| now.duration_since(ts) >= self.ttl_from_ema(self.ema_bid_interval, cfg))
                .unwrap_or(false)
            && self.updates_since_bid_change >= cfg.min_updates
    }

    fn note_crossed(&mut self, crossed: bool) {
        if crossed {
            self.crossed_frames = self.crossed_frames.saturating_add(1);
        } else {
            self.crossed_frames = 0;
        }
    }

    fn should_escalate(&self, cfg: &TobGuardConfig) -> bool {
        self.crossed_frames >= cfg.max_cross_frames
    }

    fn mark_fresh(&mut self, now: Instant, bid: f64, ask: f64) {
        self.last_bid = Some(bid);
        self.last_ask = Some(ask);
        self.last_bid_changed_at = Some(now);
        self.last_ask_changed_at = Some(now);
        self.updates_since_bid_change = 0;
        self.updates_since_ask_change = 0;
        self.crossed_frames = 0;
    }
}

// --- Transport helpers ---------------------------------------------------------------

struct Sockets {
    order: WsStream,
    stats: WsStream,
    account: WsStream,
    trade: WsStream,
    tx: WsConnection,
}

impl Sockets {
    async fn connect(client: &LighterClient, account_index: i64, auth_token: &str) -> Result<Self> {
        Ok(Self {
            order: connect_order_book_stream(client).await?,
            stats: connect_market_stats_stream(client).await?,
            account: connect_account_orders_stream(client, account_index, auth_token).await?,
            trade: connect_trade_stream(client).await?,
            tx: connect_transactions_stream(client, auth_token).await?,
        })
    }

    async fn rebuild_all(
        &mut self,
        client: &LighterClient,
        account_index: i64,
        auth_token: &str,
    ) -> Result<()> {
        self.order = connect_order_book_stream(client).await?;
        self.stats = connect_market_stats_stream(client).await?;
        self.account = connect_account_orders_stream(client, account_index, auth_token).await?;
        self.trade = connect_trade_stream(client).await?;
        self.tx = connect_transactions_stream(client, auth_token).await?;
        Ok(())
    }
}

// --- Main ----------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    banner();

    let private_key = std::env::var("LIGHTER_PRIVATE_KEY").context("LIGHTER_PRIVATE_KEY not set")?;
    let account_index: i64 = std::env::var("LIGHTER_ACCOUNT_INDEX")
        .or_else(|_| std::env::var("ACCOUNT_INDEX"))
        .context("LIGHTER_ACCOUNT_INDEX or ACCOUNT_INDEX not set")?
        .parse()
        .context("Failed to parse account index")?;
    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")
        .unwrap_or_else(|_| "0".to_string())
        .parse()
        .context("Failed to parse LIGHTER_API_KEY_INDEX")?;
    let api_url = std::env::var("LIGHTER_API_URL")
        .unwrap_or_else(|_| "https://mainnet.zklighter.elliot.ai".to_string());

    let client = LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await
        .context("Failed to build LighterClient")?;

    let signer = client
        .signer()
        .context("Client not configured with signer (private key missing)?")?;

    log_action("Connected REST + signer ready");

    let details = client
        .orders()
        .book_details(Some(MarketId::new(MARKET_ID)))
        .await
        .context("Failed to load market metadata")?
        .order_book_details
        .into_iter()
        .find(|d| d.market_id == MARKET_ID)
        .context("Market metadata missing for requested market")?;

    let price_multiplier = 10_i64
        .checked_pow(details.price_decimals as u32)
        .context("price_decimals overflow")?;
    let size_multiplier = 10_i64
        .checked_pow(details.size_decimals as u32)
        .context("size_decimals overflow")?;
    let tick_size = 1.0 / price_multiplier as f64;
    let min_clip = safe_parse(&details.min_base_amount)
        .unwrap_or(1.0 / size_multiplier as f64);

    let auth_token = client.create_auth_token(None)?;
    let mut sockets = Sockets::connect(&client, account_index, &auth_token).await?;

    let guard_cfg = TobGuardConfig::default();

    let dry_run = std::env::var(DRY_RUN_ENV)
        .ok()
        .map(|value| value.to_ascii_lowercase())
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    if dry_run {
        log_action("🥽  DRY RUN ENABLED — orders will not be submitted");
    }

    let simulate_fills = std::env::var(SIMULATE_FILLS_ENV)
        .ok()
        .map(|value| value.to_ascii_lowercase())
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    if simulate_fills {
        log_action("🎮  FILL SIMULATION ENABLED — will simulate inventory accumulation for testing");
    }

    let mut generation = 0usize;
    let session_start = Instant::now();

    let mut hawkes_bid = HawkesSide::new(HAWKES_MU_BID, HAWKES_ALPHA_BID, HAWKES_BETA_BID, 0.0);
    let mut hawkes_ask = HawkesSide::new(HAWKES_MU_ASK, HAWKES_ALPHA_ASK, HAWKES_BETA_ASK, 0.0);
    let quote_params = QuoteMapParams {
        k_bid: K_BID,
        k_ask: K_ASK,
        lam_star_bid: LAMBDA_STAR_BID,
        lam_star_ask: LAMBDA_STAR_ASK,
        min_ticks: MIN_TICKS,
        max_ticks: MAX_TICKS,
        c_size: SIZE_DAMP_C,
        q_max: Q_MAX,
        min_clip,
    };

    let mut last_order_message = Instant::now();
    let mut last_stats_message = Instant::now();
    let mut last_account_message = Instant::now();
    let mut last_trade_message = Instant::now();
    let mut last_tx_message = Instant::now();
    let mut last_refresh = Instant::now() - MIN_REFRESH_INTERVAL;
    let mut last_mark: Option<(f64, Instant)> = None;
    let mut market_watchdog = MarketWatchdog::new();
    let mut tob_guard = TobGuard::new();
    let mut last_rest_refresh: Option<Instant> = None;
    let mut tracked_orders: HashMap<i64, OrderTrackerEntry> = HashMap::new();
    let mut inventory: f64 = 0.0;
    let mut client_order_seq: i64 = 1;
    let mut active_orders: ActiveOrders = ActiveOrders::default();
    let mut pending_orders: PendingOrders = PendingOrders::default();
    let mut markout_tracker = MarkOutTracker::new(MARKOUT_MEDIAN_WINDOW);
    let mut crash_detector = CrashDetector::new(tick_size);
    let mut crash_mode = false;
    let mut crash_trim_pending = false;
    let mut fill_breaker = FillBreaker::new();
    let mut quote_cache = QuoteCache::default();
    let mut telemetry = Telemetry::new();
    let mut vol_estimator = VolatilityEstimator::new(VOL_EWMA_ALPHA, VOL_BAR_DURATION);
    let ofi_weight = std::env::var("OFI_ALPHA_WEIGHT")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(OFI_DEFAULT_WEIGHT);
    let mut ofi_tracker = OfiTracker::new(ofi_weight, OFI_DECAY_ALPHA);
    let mut simulation_counter: u64 = 0;  // For fill simulation

    let mut order_watchdog = interval(Duration::from_millis(200));
    order_watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut stats_watchdog = interval(Duration::from_millis(500));
    stats_watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut account_watchdog = interval(Duration::from_millis(500));
    account_watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut trade_watchdog = interval(Duration::from_millis(500));
    trade_watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut tx_watchdog = interval(Duration::from_millis(500));
    tx_watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut tob_issues = 0u64;

    'outer: loop {
        generation += 1;
        log_action(&format!("🔁 Generation #{generation} running"));

        'event_loop: loop {
            tokio::select! {
                _ = order_watchdog.tick() => {
                    let now = Instant::now();
                    if now.duration_since(last_order_message) > MAX_NO_MESSAGE_TIME {
                        log_action("⚠️  Order-book socket idle; reconnecting");
                        if let Err(err) = sockets.order.connection_mut().reconnect(Some(3)).await {
                            log_action(&format!("❌  Order-book reconnect failed: {err}"));
                            break 'event_loop;
                        }
                        last_order_message = Instant::now();
                        market_watchdog.mark_recovered();
                    }
                }
                _ = stats_watchdog.tick() => {
                    let now = Instant::now();
                    if now.duration_since(last_stats_message) > MAX_NO_MESSAGE_TIME {
                        log_action("⚠️  Market-stats socket idle; reconnecting");
                        if let Err(err) = sockets.stats.connection_mut().reconnect(Some(3)).await {
                            log_action(&format!("❌  Market-stats reconnect failed: {err}"));
                            break 'event_loop;
                        }
                        last_stats_message = Instant::now();
                    }
                }
                _ = account_watchdog.tick() => {
                    let now = Instant::now();
                    if now.duration_since(last_account_message) > ACCOUNT_IDLE_GRACE {
                        log_action("⚠️  Account socket idle; reconnecting");
                        if let Err(err) = sockets.account.connection_mut().reconnect(Some(3)).await {
                            log_action(&format!("❌  Account reconnect failed: {err}"));
                            break 'event_loop;
                        }
                        last_account_message = Instant::now();
                    }
                }
                _ = trade_watchdog.tick() => {
                    let now = Instant::now();
                    if now.duration_since(last_trade_message) > MAX_NO_MESSAGE_TIME {
                        log_action("⚠️  Trade socket idle; reconnecting");
                        if let Err(err) = sockets.trade.connection_mut().reconnect(Some(3)).await {
                            log_action(&format!("❌  Trade reconnect failed: {err}"));
                            break 'event_loop;
                        }
                        last_trade_message = Instant::now();
                    }
                }
                _ = tx_watchdog.tick() => {
                    let now = Instant::now();
                    if now.duration_since(last_tx_message) > MAX_NO_MESSAGE_TIME {
                        log_action("⚠️  Tx socket idle; reconnecting");
                        if let Err(err) = sockets.tx.reconnect(Some(3)).await {
                            log_action(&format!("❌  Tx reconnect failed: {err}"));
                            break 'event_loop;
                        }
                        last_tx_message = Instant::now();
                    }
                }
                evt = sockets.order.next() => {
                    match evt {
                        Some(Ok(WsEvent::OrderBook(ob))) => {
                            let now = Instant::now();
                            last_order_message = now;

                            let now_secs = session_start.elapsed().as_secs_f64();

                            let mut used_rest_snapshot = false;
                            let (best_bid, best_ask, skipped_level) = match derive_bbo_guarded(
                                now,
                                &ob.state,
                                &guard_cfg,
                                &mut tob_guard,
                            ) {
                                GuardOutcome::Bbo { bid, ask, skipped } => (bid, ask, skipped),
                                GuardOutcome::Escalate { reason } => {
                                    let mut fallback = None;
                                    if REST_REFRESH_ENABLED
                                        && last_rest_refresh
                                            .map(|ts| now.duration_since(ts) >= REST_REFRESH_COOLDOWN)
                                            .unwrap_or(true)
                                    {
                                        match fetch_rest_bbo(&client).await {
                                            Ok(Some((bid, ask))) => {
                                                log_action(&format!(
                                                    "⚠️  {reason}; refreshed BBO via REST snapshot bid={bid:.4} ask={ask:.4}"
                                                ));
                                                tob_guard.mark_fresh(now, bid, ask);
                                                last_rest_refresh = Some(now);
                                                used_rest_snapshot = true;
                                                fallback = Some((bid, ask));
                                            }
                                            Ok(None) => {
                                                log_action("⚠️  REST snapshot returned empty book; rebuilding sockets");
                                            }
                                            Err(err) => {
                                                log_action(&format!("⚠️  REST snapshot failed: {err}; rebuilding sockets"));
                                            }
                                        }
                                    }
                                    if let Some((bid, ask)) = fallback {
                                        (bid, ask, true)
                                    } else {
                                        break 'event_loop;
                                    }
                                }
                            };

                            if used_rest_snapshot {
                                log_action(&format!(
                                    "ℹ️  Using REST snapshot BBO bid={best_bid:.4} ask={best_ask:.4}"
                                ));
                            } else if skipped_level {
                                tob_issues += 1;
                                if tob_issues % 8 == 0 {
                                    log_action("⚠️  Stale top level filtered; using deeper price");
                                }
                            }

                            let best_bid_ticks = (best_bid / tick_size).floor() as i64;
                            let best_ask_ticks = (best_ask / tick_size).ceil() as i64;

                            let best_bid_size = ob
                                .state
                                .bids
                                .iter()
                                .find(|lvl| level_active(lvl))
                                .map(level_quantity)
                                .unwrap_or(0.0);
                            let best_ask_size = ob
                                .state
                                .asks
                                .iter()
                                .find(|lvl| level_active(lvl))
                                .map(level_quantity)
                                .unwrap_or(0.0);

                            ofi_tracker.on_book_update(&ob.state);

                            match market_watchdog.observe(best_bid_ticks, best_ask_ticks, now) {
                                WatchdogDecision::None => {}
                                WatchdogDecision::Reconnect { delay, reason, attempts, escalate } => {
                                    log_action(&format!(
                                        "⚠️  Watchdog reconnecting order_book/{} (reason={}, attempts_last_min={})",
                                        MARKET_ID,
                                        reason.as_str(),
                                        attempts
                                    ));
                                    if escalate {
                                        break 'event_loop;
                                    }
                                    if !delay.is_zero() {
                                        sleep(delay).await;
                                    }
                                    if let Err(err) = sockets.order.connection_mut().reconnect(Some(3)).await {
                                        log_action(&format!("❌  Order-book reconnect failed: {err}"));
                                        break 'event_loop;
                                    }
                                    last_order_message = Instant::now();
                                    market_watchdog.mark_recovered();
                                    continue;
                                }
                            }

                            if best_ask <= best_bid + PRICE_EPS {
                                log_action(&format!(
                                    "⚠️  Skipping crossed snapshot bid={best_bid:.4} ask={best_ask:.4}"
                                ));
                                continue;
                            }

                            if last_refresh.elapsed() < MIN_REFRESH_INTERVAL {
                                continue;
                            }

                            let raw_mid = 0.5 * (best_bid + best_ask);
                            vol_estimator.update(raw_mid, now);
                            if let Some(median) = markout_tracker.update(now, raw_mid) {
                                if median < 0.0 {
                                    log_action(&format!(
                                        "⚠️  Median 1s mark-out {:.6}; widening heavy side",
                                        median
                                    ));
                                }
                            }

                            let ofi_bps = ofi_tracker.adjustment_bps();
                            let ofi_price_shift = raw_mid * (ofi_bps * 0.0001);
                            let mid = (raw_mid + ofi_price_shift).max(tick_size);

                            let mark = last_mark
                                .filter(|(_, ts)| ts.elapsed() <= MARK_FRESH_FOR)
                                .map(|(m, _)| m)
                                .unwrap_or(raw_mid);
                            let mark = (mark + ofi_price_shift).max(tick_size);

                            let crash_signal =
                                crash_detector.update(now, mark, best_bid, best_ask, &ob.state);
                            match crash_signal {
                                CrashSignal::Enter => {
                                    if !crash_mode {
                                        crash_mode = true;
                                        crash_trim_pending = true;
                                        log_action("⚠️  Crash detector triggered; entering defensive mode");
                                    }
                                }
                                CrashSignal::Maintain => {
                                    if !crash_mode {
                                        crash_mode = true;
                                    }
                                }
                                CrashSignal::Exit => {
                                    if crash_mode {
                                        crash_mode = false;
                                        log_action("✅  Crash cooldown complete; resuming normal mode");
                                    }
                                }
                                CrashSignal::None => {}
                            }

                            if crash_mode && crash_trim_pending && !dry_run {
                                match submit_crash_trim_orders(
                                    &client,
                                    signer,
                                    &mut sockets.tx,
                                    inventory,
                                    size_multiplier,
                                    best_bid_ticks,
                                    best_ask_ticks,
                                )
                                .await
                                {
                                    Ok(sent) => {
                                        if sent {
                                            log_action("⚠️  Submitted crash trim IOC orders");
                                        }
                                    }
                                    Err(err) => {
                                        log_action(&format!(
                                            "⚠️  Failed to submit crash trim orders: {err}"
                                        ));
                                    }
                                }
                                crash_trim_pending = false;
                            }

                            let mut decision = decide_quotes(
                                mark,
                                now_secs,
                                inventory,
                                &mut hawkes_bid,
                                &mut hawkes_ask,
                                &quote_params,
                                tick_size,
                            );

                            if crash_mode {
                                if inventory > 0.0 {
                                    decision.bid_qty = 0.0;
                                } else if inventory < 0.0 {
                                    decision.ask_qty = 0.0;
                                }
                                decision.bid_qty *= CRASH_SIZE_FACTOR;
                                decision.ask_qty *= CRASH_SIZE_FACTOR;
                                let crash_half =
                                    (mark * (CRASH_HALF_SPREAD_BPS * 0.0001)).max(3.0 * tick_size);
                                decision.bid_price = (mark - crash_half).max(tick_size);
                                decision.ask_price = mark + crash_half;
                            }

                            if !fill_breaker.should_quote(QuoteSide::Bid, now) {
                                decision.bid_qty = 0.0;
                            }
                            if !fill_breaker.should_quote(QuoteSide::Ask, now) {
                                decision.ask_qty = 0.0;
                            }

                            let half_spread = (mark * (TARGET_EDGE_BPS * 0.0001)).max(mark * BASE_SPREAD_PCT * 0.5);
                            let floor_dist = (2.0 * MAKER_FEE_PCT + TARGET_EDGE_BPS * 0.0001) * mark * 0.5;
                            let dist_floor = (floor_dist / tick_size).ceil() * tick_size;
                            // Use wider of: inventory-adjusted price OR minimum floor distance
                            let mut bid_price = decision.bid_price.min(mark - dist_floor);
                            let mut ask_price = decision.ask_price.max(mark + dist_floor);

                            if ask_price - bid_price < 2.0 * half_spread {
                                let adjust = 0.5 * (2.0 * half_spread - (ask_price - bid_price));
                                bid_price -= adjust;
                                ask_price += adjust;
                            }

                            let raw_vol_bps = vol_estimator.volatility_bps();
                            let vol_spread_bps = raw_vol_bps * VOL_SPREAD_MULTIPLIER;
                            let vol_half = (vol_spread_bps * 0.0001 * mid) * 0.5;
                            if vol_half.is_finite() {
                                let required_spread = vol_half * 2.0;
                                let current_spread = ask_price - bid_price;
                                if required_spread > current_spread {
                                    let adjust = 0.5 * (required_spread - current_spread);
                                    bid_price = (bid_price - adjust).max(tick_size);
                                    ask_price += adjust;
                                }
                            }

                            let (markout_bid_bump, markout_ask_bump) =
                                markout_tracker.heavy_side_bump(inventory, tick_size);
                            let breaker_bid_bump = fill_breaker.take_extra_ticks(QuoteSide::Bid);
                            let breaker_ask_bump = fill_breaker.take_extra_ticks(QuoteSide::Ask);
                            if markout_bid_bump > 0.0 {
                                bid_price = (bid_price - markout_bid_bump).max(tick_size);
                            }
                            if markout_ask_bump > 0.0 {
                                ask_price += markout_ask_bump;
                            }
                            if breaker_bid_bump > 0 {
                                bid_price = (bid_price - breaker_bid_bump as f64 * tick_size).max(tick_size);
                            }
                            if breaker_ask_bump > 0 {
                                ask_price += breaker_ask_bump as f64 * tick_size;
                            }

                            let queue_threshold = QUEUE_THRESHOLD_MULTIPLIER * Q_MAX;
                            if decision.bid_qty > 0.0 && best_bid_size >= queue_threshold {
                                let stepped_price = bid_price + tick_size;
                                if stepped_price < best_ask - tick_size {
                                    bid_price = stepped_price;
                                    log_action("📍 Stepping bid ahead due to thick queue");
                                }
                            }
                            if decision.ask_qty > 0.0 && best_ask_size >= queue_threshold {
                                let stepped_price = (ask_price - tick_size).max(tick_size);
                                if stepped_price > best_bid + tick_size {
                                    ask_price = stepped_price;
                                    log_action("📍 Stepping ask ahead due to thick queue");
                                }
                            }

                            let bid_ticks = (bid_price / tick_size).floor() as i64;
                            let ask_ticks = (ask_price / tick_size).ceil() as i64;
                            let po_bid = bid_ticks as f64 * tick_size;
                            let po_ask = ask_ticks as f64 * tick_size;

                            // Calculate spread in basis points for debugging
                            let spread_bps = ((po_ask - po_bid) / mid) * 10000.0;
                            let bid_dist_bps = ((mid - po_bid) / mid) * 10000.0;
                            let ask_dist_bps = ((po_ask - mid) / mid) * 10000.0;

                            log_action(&format!(
                                "mid={mid:.4} mark={mark:.4} -> bid={po_bid:.4} ask={po_ask:.4} | spread={spread_bps:.1}bps (b:{bid_dist_bps:.1}/a:{ask_dist_bps:.1}) | q_bid={:.5} q_ask={:.5} | λb={:.2} λa={:.2} | inv={inventory:.6}",
                                decision.bid_qty,
                                decision.ask_qty,
                                decision.lambda_bid,
                                decision.lambda_ask
                            ));

                            let mut desired_bid = if decision.bid_qty > 0.0 {
                                let units = qty_to_units(decision.bid_qty, size_multiplier);
                                if units > 0 {
                                    Some(DesiredQuote {
                                        price_ticks: bid_ticks,
                                        base_qty_units: units,
                                    })
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            let mut desired_ask = if decision.ask_qty > 0.0 {
                                let units = qty_to_units(decision.ask_qty, size_multiplier);
                                if units > 0 {
                                    Some(DesiredQuote {
                                        price_ticks: ask_ticks,
                                        base_qty_units: units,
                                    })
                                } else {
                                    None
                                }
                            } else {
                                None
                            };

                            let ladder_quotes = create_exit_ladder(
                                inventory,
                                mark,
                                tick_size,
                                size_multiplier,
                            );

                            if inventory >= HARD_POS_CAP {
                                if desired_bid.is_some() {
                                    log_action("⚠️  Inventory above hard cap; CANCELING BID to prevent further buys");
                                    desired_bid = None;  // Cancel bid when long above hard cap
                                }
                            } else if inventory >= SOFT_POS_CAP {
                                if desired_bid.is_some() {
                                    log_action("⚠️  Inventory above soft cap; quoting defensively on bid");
                                }
                            }

                            if inventory <= -HARD_POS_CAP {
                                if desired_ask.is_some() {
                                    log_action("⚠️  Inventory below hard cap; CANCELING ASK to prevent further sells");
                                    desired_ask = None;  // Cancel ask when short below hard cap
                                }
                            } else if inventory <= -SOFT_POS_CAP {
                                if desired_ask.is_some() {
                                    log_action("⚠️  Inventory below soft cap; quoting defensively on ask");
                                }
                            }

                            if crash_mode {
                                if inventory > 0.0 {
                                    desired_bid = None;
                                }
                                if inventory < 0.0 {
                                    desired_ask = None;
                                }
                            }

                            if dry_run {
                                // Simulate fills for testing if enabled
                                if simulate_fills {
                                    simulation_counter += 1;

                                    // Simulate fills every 10 updates (about 1 second)
                                    if simulation_counter % 10 == 0 {
                                        // Build short position to test ask widening
                                        let fill_size = 0.001;  // 0.001 BTC per fill
                                        inventory -= fill_size;  // Always sell to build short position

                                        // Cap for safety
                                        inventory = inventory.clamp(-HARD_POS_CAP, HARD_POS_CAP);

                                        log_action(&format!(
                                            "🎮  SIMULATED FILL: SELL {:.4} BTC | New inventory: {:.6} BTC ({:.1}% of soft cap)",
                                            fill_size,
                                            inventory,
                                            (inventory.abs() / SOFT_POS_CAP) * 100.0
                                        ));

                                        // Also trigger Hawkes intensity
                                        hawkes_ask.on_event(now_secs);
                                    }
                                }

                                last_refresh = Instant::now();
                                continue;
                            }

                            let now = Instant::now();
                            telemetry.on_position(now, inventory);

                            let mut update_bid = quote_cache.should_update_bid(&desired_bid);
                            let mut update_ask = quote_cache.should_update_ask(&desired_ask);
                            let ladder_needed = !ladder_quotes.is_empty();

                            if ladder_needed {
                                if desired_bid.is_some() {
                                    update_bid = true;
                                }
                                if desired_ask.is_some() {
                                    update_ask = true;
                                }
                            }

                            if !update_bid && !update_ask && !ladder_needed {
                                last_refresh = Instant::now();
                                telemetry.report(
                                    now,
                                    inventory,
                                    decision.lambda_bid,
                                    decision.lambda_ask,
                                    markout_tracker.median(),
                                    ofi_bps,
                                    raw_vol_bps,
                                );
                                continue;
                            }

                            let mut batch: Vec<(u8, String)> = Vec::new();
                            if update_bid || update_ask || ladder_needed {
                                let (api_key, nonce) = signer.next_nonce().await?;
                                let payload = signer
                                    .sign_cancel_all_orders(0, 0, Some(nonce), Some(api_key))
                                    .await
                                    .context("failed to sign cancel-all")?;
                                batch.push((TX_TYPE_CANCEL_ALL_ORDERS, payload));
                            }

                            if update_bid {
                                if let Some(desired) = desired_bid.as_ref() {
                                    let qty = units_to_qty(desired.base_qty_units, size_multiplier);
                                    if qty > 0.0 {
                                        let base_qty = qty_to_base(qty, size_multiplier)?;
                                        let (api_key, nonce) = signer.next_nonce().await?;
                                        let mut builder = client
                                            .order(MarketId::new(MARKET_ID))
                                            .buy()
                                            .qty(base_qty)
                                            .limit(Price::ticks(desired.price_ticks));
                                        if crash_mode {
                                            builder = builder.reduce_only();
                                        }
                                        builder = builder.post_only();
                                        let payload = builder
                                            .with_api_key(ApiKeyIndex::new(api_key))
                                            .with_nonce(Nonce::new(nonce))
                                            .with_client_order_id(client_order_seq)
                                            .sign()
                                            .await
                                            .context("failed to sign bid order")?
                                            .payload()
                                            .to_string();
                                        batch.push((TX_TYPE_CREATE_ORDER, payload));
                                        pending_orders.mark(QuoteSide::Bid, desired.price_ticks, desired.base_qty_units, now);
                                        client_order_seq += 1;
                                    }
                                    quote_cache.record_bid(&desired_bid);
                                } else {
                                    pending_orders.clear(QuoteSide::Bid);
                                    quote_cache.record_bid(&desired_bid);
                                }
                            }

                            if update_ask {
                                if let Some(desired) = desired_ask.as_ref() {
                                    let qty = units_to_qty(desired.base_qty_units, size_multiplier);
                                    if qty > 0.0 {
                                        let base_qty = qty_to_base(qty, size_multiplier)?;
                                        let (api_key, nonce) = signer.next_nonce().await?;
                                        let mut builder = client
                                            .order(MarketId::new(MARKET_ID))
                                            .sell()
                                            .qty(base_qty)
                                            .limit(Price::ticks(desired.price_ticks));
                                        if crash_mode {
                                            builder = builder.reduce_only();
                                        }
                                        builder = builder.post_only();
                                        let payload = builder
                                            .with_api_key(ApiKeyIndex::new(api_key))
                                            .with_nonce(Nonce::new(nonce))
                                            .with_client_order_id(client_order_seq)
                                            .sign()
                                            .await
                                            .context("failed to sign ask order")?
                                            .payload()
                                            .to_string();
                                        batch.push((TX_TYPE_CREATE_ORDER, payload));
                                        pending_orders.mark(QuoteSide::Ask, desired.price_ticks, desired.base_qty_units, now);
                                        client_order_seq += 1;
                                    }
                                    quote_cache.record_ask(&desired_ask);
                                } else {
                                    pending_orders.clear(QuoteSide::Ask);
                                    quote_cache.record_ask(&desired_ask);
                                }
                            }

                            if ladder_needed {
                                for (side, quote) in ladder_quotes {
                                    let qty = units_to_qty(quote.base_qty_units, size_multiplier);
                                    if qty <= 0.0 {
                                        continue;
                                    }
                                    let base_qty = qty_to_base(qty, size_multiplier)?;
                                    let (api_key, nonce) = signer.next_nonce().await?;
                                    let builder = match side {
                                        QuoteSide::Bid => client.order(MarketId::new(MARKET_ID)).buy(),
                                        QuoteSide::Ask => client.order(MarketId::new(MARKET_ID)).sell(),
                                    }
                                    .qty(base_qty)
                                    .limit(Price::ticks(quote.price_ticks))
                                    .post_only()
                                    .reduce_only();
                                    let payload = builder
                                        .with_api_key(ApiKeyIndex::new(api_key))
                                        .with_nonce(Nonce::new(nonce))
                                        .with_client_order_id(client_order_seq)
                                        .sign()
                                        .await
                                        .context("failed to sign ladder order")?
                                        .payload()
                                        .to_string();
                                    batch.push((TX_TYPE_CREATE_ORDER, payload));
                                    log_action(&format!(
                                        "🪜 Ladder {:?} rung qty={qty:.5} at ticks {}",
                                        side, quote.price_ticks
                                    ));
                                    client_order_seq += 1;
                                }
                            }

                            if batch.is_empty() {
                                last_refresh = Instant::now();
                                continue;
                            }

                            let batch_for_send = batch.clone();
                            let start = Instant::now();
                            match send_batch_tx_ws(&mut sockets.tx, batch_for_send).await {
                                Ok(results) => {
                                    for ((tx_type, _), success) in batch.iter().zip(results.iter()) {
                                        if *tx_type == TX_TYPE_CREATE_ORDER {
                                            telemetry.on_post_only_response(*success);
                                        }
                                    }
                                    log_action(&format!(
                                        "Batch ok ({} ops, {:?})",
                                        results.len(),
                                        start.elapsed()
                                    ));
                                    last_tx_message = Instant::now();
                                }
                                Err(err) => {
                                    log_action(&format!("❌  Batch submission failed: {err}; reconnecting tx socket"));
                                    pending_orders.clear(QuoteSide::Bid);
                                    pending_orders.clear(QuoteSide::Ask);
                                    for (tx_type, _) in batch.iter() {
                                        if *tx_type == TX_TYPE_CREATE_ORDER {
                                            telemetry.on_post_only_response(false);
                                        }
                                    }
                                    if let Err(conn_err) = sockets.tx.reconnect(Some(3)).await {
                                        log_action(&format!("❌  Tx reconnect failed: {conn_err}"));
                                        break 'event_loop;
                                    }
                                    last_tx_message = Instant::now();
                                }
                            }

                            active_orders.bid = None;
                            active_orders.ask = None;
                            last_refresh = Instant::now();

                            telemetry.report(
                                now,
                                inventory,
                                decision.lambda_bid,
                                decision.lambda_ask,
                                markout_tracker.median(),
                                ofi_bps,
                                raw_vol_bps,
                            );
                        }
                        Some(Ok(WsEvent::Connected)) => {
                            last_order_message = Instant::now();
                            log_action("Order-book socket connected");
                        }
                        Some(Ok(event)) => {
                            if matches!(event, WsEvent::Pong) {
                                last_order_message = Instant::now();
                            }
                        }
                        Some(Err(err)) => {
                            log_action(&format!("❌ Order-book socket error: {err}"));
                            break 'event_loop;
                        }
                        None => {
                            log_action("⚠️  Order-book socket closed by server");
                            break 'event_loop;
                        }
                    }
                }
                evt = sockets.stats.next() => {
                    match evt {
                        Some(Ok(WsEvent::MarketStats(stats))) => {
                            last_stats_message = Instant::now();
                            if let Some(mark) = safe_parse(&stats.market_stats.mark_price) {
                                last_mark = Some((mark, Instant::now()));
                            }
                        }
                        Some(Ok(WsEvent::Connected)) => {
                            last_stats_message = Instant::now();
                            log_action("Market-stats socket connected");
                        }
                        Some(Ok(event)) => {
                            if matches!(event, WsEvent::Pong) {
                                last_stats_message = Instant::now();
                            }
                        }
                        Some(Err(err)) => {
                            log_action(&format!("❌ Market-stats socket error: {err}"));
                            break 'event_loop;
                        }
                        None => {
                            log_action("⚠️  Market-stats socket closed by server");
                            break 'event_loop;
                        }
                    }
                }
                evt = sockets.account.next() => {
                    match evt {
                        Some(Ok(WsEvent::Account(envelope))) => {
                            last_account_message = Instant::now();
                            process_account_event(
                                envelope,
                                session_start.elapsed().as_secs_f64(),
                                &mut hawkes_bid,
                                &mut hawkes_ask,
                                &mut tracked_orders,
                                &mut inventory,
                                &mut active_orders,
                                &mut pending_orders,
                                &mut markout_tracker,
                                &mut fill_breaker,
                                &mut telemetry,
                                size_multiplier,
                                tick_size,
                            );
                        }
                        Some(Ok(WsEvent::Connected)) => {
                            last_account_message = Instant::now();
                            log_action("Account socket connected");
                        }
                        Some(Ok(event)) => {
                            if matches!(event, WsEvent::Pong) {
                                last_account_message = Instant::now();
                            }
                        }
                        Some(Err(err)) => {
                            log_action(&format!("❌ Account socket error: {err}"));
                            break 'event_loop;
                        }
                        None => {
                            log_action("⚠️  Account socket closed by server");
                            break 'event_loop;
                        }
                    }
                }
                evt = sockets.trade.next() => {
                    match evt {
                        Some(Ok(WsEvent::Trade(trade_event))) => {
                            last_trade_message = Instant::now();
                            process_trade_event(
                                trade_event,
                                session_start.elapsed().as_secs_f64(),
                                &mut hawkes_bid,
                                &mut hawkes_ask,
                            );
                        }
                        Some(Ok(WsEvent::Connected)) => {
                            last_trade_message = Instant::now();
                            log_action("Trade socket connected");
                        }
                        Some(Ok(event)) => {
                            if matches!(event, WsEvent::Pong) {
                                last_trade_message = Instant::now();
                            }
                        }
                        Some(Err(err)) => {
                            log_action(&format!("❌ Trade socket error: {err}"));
                            break 'event_loop;
                        }
                        None => {
                            log_action("⚠️  Trade socket closed by server");
                            break 'event_loop;
                        }
                    }
                }
                evt = sockets.tx.next_event() => {
                    match evt {
                        Ok(Some(event)) => {
                            last_tx_message = Instant::now();
                            handle_tx_event(event);
                        }
                        Ok(None) => {
                            log_action("⚠️  Tx connection closed by server");
                            break 'event_loop;
                        }
                        Err(err) => {
                            log_action(&format!("❌ Tx stream error: {err}"));
                            break 'event_loop;
                        }
                    }
                }
            }
        }

        log_action("🔄  Rebuilding sockets after loop exit");
        if let Err(err) = sockets.rebuild_all(&client, account_index, &auth_token).await {
            log_action(&format!("❌  Failed to rebuild sockets: {err}"));
            break 'outer;
        }
        market_watchdog.mark_recovered();
        let now = Instant::now();
        last_order_message = now;
        last_stats_message = now;
        last_account_message = now;
        last_trade_message = now;
        last_tx_message = now;
    }

    Ok(())
}

// --- Quoting decisions ---------------------------------------------------------------

fn decide_quotes(
    mid: f64,
    now: f64,
    inventory: f64,
    hawkes_bid: &mut HawkesSide,
    hawkes_ask: &mut HawkesSide,
    params: &QuoteMapParams,
    tick: f64,
) -> QuoteDecision {
    let lam_b = hawkes_bid.value(now);
    let lam_a = hawkes_ask.value(now);

    let mut d_bid = (lam_b / params.lam_star_bid).ln() / params.k_bid;
    let mut d_ask = (lam_a / params.lam_star_ask).ln() / params.k_ask;

    let k_bar = 0.5 * (params.k_bid + params.k_ask);
    let skew = 0.5 * (lam_b / lam_a).ln() / k_bar;
    d_bid = (d_bid + skew).clamp(params.min_ticks * tick, params.max_ticks * tick);
    d_ask = (d_ask - skew).clamp(params.min_ticks * tick, params.max_ticks * tick);

    let inv_ratio = (inventory / SOFT_POS_CAP).clamp(-1.0, 1.0);
    let inv_mag = inv_ratio.abs();
    if inv_mag > PRICE_EPS {
        let skew_bps = (MAX_INV_SKEW_BPS * inv_mag * inv_mag).min(MAX_INV_SKEW_BPS);
        let skew_price = mid * (skew_bps * 0.0001);

        if inv_ratio > 0.0 {
            // Long inventory: widen BIDS (bid fills = buy more = BAD)
            // "Lean away from the heavy side"
            d_bid = d_bid + skew_price;
        } else if inv_ratio < 0.0 {
            // Short inventory: widen ASKS (ask fills = sell more = BAD)
            // "Lean away from the heavy side"
            d_ask = d_ask + skew_price;
        }
    }

    let q_base_bid = params.q_max / (1.0 + params.c_size * lam_a);
    let q_base_ask = params.q_max / (1.0 + params.c_size * lam_b);

    let size_ratio = (inventory.abs() / SOFT_POS_CAP).min(1.5);
    let size_multiplier = if size_ratio < 0.5 {
        1.0
    } else if size_ratio < 1.0 {
        1.0 - 0.3 * ((size_ratio - 0.5) * 2.0)
    } else {
        0.7 - 0.2 * ((size_ratio - 1.0) / 0.5).min(1.0)
    };

    let min_qty = params.min_clip.max(Q_MIN_EXCHANGE);
    let q_bid = if inventory > 0.0 {
        (q_base_bid * size_multiplier).max(min_qty).min(params.q_max)
    } else {
        q_base_bid.max(min_qty).min(params.q_max)
    };
    let q_ask = if inventory < 0.0 {
        (q_base_ask * size_multiplier).max(min_qty).min(params.q_max)
    } else {
        q_base_ask.max(min_qty).min(params.q_max)
    };

    QuoteDecision {
        bid_price: (mid - d_bid).max(tick),
        ask_price: mid + d_ask,
        bid_qty: q_bid,
        ask_qty: q_ask,
        lambda_bid: lam_b,
        lambda_ask: lam_a,
    }
}

fn qty_to_base(qty: f64, size_multiplier: i64) -> Result<BaseQty> {
    let raw = (qty * size_multiplier as f64).round() as i64;
    BaseQty::try_from(raw).map_err(|_| anyhow!("quantity rounded to zero"))
}

fn qty_to_units(qty: f64, size_multiplier: i64) -> i64 {
    (qty * size_multiplier as f64).round() as i64
}

fn units_to_qty(units: i64, size_multiplier: i64) -> f64 {
    units as f64 / size_multiplier as f64
}

// --- Account processing --------------------------------------------------------------

fn process_account_event(
    envelope: AccountEventEnvelope,
    now_secs: f64,
    hawkes_bid: &mut HawkesSide,
    hawkes_ask: &mut HawkesSide,
    tracker: &mut HashMap<i64, OrderTrackerEntry>,
    inventory: &mut f64,
    active_orders: &mut ActiveOrders,
    pending: &mut PendingOrders,
    markouts: &mut MarkOutTracker,
    breaker: &mut FillBreaker,
    telemetry: &mut Telemetry,
    size_multiplier: i64,
    tick_size: f64,
) {
    let value = envelope.event.as_value();

    if let Some(positions_obj) = value.get("positions").and_then(|v| v.as_object()) {
        sync_inventory_from_positions(positions_obj, inventory);
    }

    if value.get("orders").is_some() {
        process_orders_payload(
            value,
            now_secs,
            hawkes_bid,
            hawkes_ask,
            tracker,
            inventory,
            active_orders,
            pending,
            markouts,
            breaker,
            telemetry,
            size_multiplier,
            tick_size,
        );
    }
}

fn process_orders_payload(
    value: &serde_json::Value,
    now_secs: f64,
    hawkes_bid: &mut HawkesSide,
    hawkes_ask: &mut HawkesSide,
    tracker: &mut HashMap<i64, OrderTrackerEntry>,
    inventory: &mut f64,
    active_orders: &mut ActiveOrders,
    pending: &mut PendingOrders,
    markouts: &mut MarkOutTracker,
    breaker: &mut FillBreaker,
    telemetry: &mut Telemetry,
    size_multiplier: i64,
    tick_size: f64,
) {
    let now_instant = Instant::now();
    let Some(orders_obj) = value.get("orders").and_then(|v| v.as_object()) else {
        active_orders.bid = None;
        active_orders.ask = None;
        return;
    };
    let market_key = MARKET_ID.to_string();
    let Some(orders) = orders_obj.get(&market_key).and_then(|v| v.as_array()) else {
        active_orders.bid = None;
        active_orders.ask = None;
        return;
    };

    let mut seen_indices = HashMap::new();
    let mut latest_bid: Option<SideOrderState> = None;
    let mut latest_ask: Option<SideOrderState> = None;

    for order_val in orders {
        let order_index = order_val
            .get("order_index")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        if order_index < 0 {
            continue;
        }

        let is_ask_raw = order_val
            .get("is_ask")
            .and_then(|v| v.as_bool());

        // DEBUG: Log the raw is_ask value to understand the API
        if order_index % 10 == 0 {  // Sample logging to avoid spam
            eprintln!("🔍 ORDER DEBUG: order_index={} is_ask={:?} price={:?}",
                     order_index, is_ask_raw,
                     order_val.get("price").and_then(|v| v.as_str()));
        }

        let side = is_ask_raw
            .map(|is_ask| if is_ask { QuoteSide::Ask } else { QuoteSide::Bid })
            .unwrap_or(QuoteSide::Bid);

        let remaining = order_val
            .get("remaining_base_amount")
            .and_then(|v| v.as_str())
            .and_then(|s| safe_parse(s))
            .or_else(|| order_val.get("size").and_then(|v| v.as_str()).and_then(|s| safe_parse(s)))
            .unwrap_or(0.0);
        let price = order_val
            .get("price")
            .and_then(|v| v.as_str())
            .and_then(|s| safe_parse(s))
            .unwrap_or(0.0);
        let price_ticks = match side {
            QuoteSide::Bid => (price / tick_size).floor() as i64,
            QuoteSide::Ask => (price / tick_size).ceil() as i64,
        };
        let base_qty_units = (remaining * size_multiplier as f64).round() as i64;

        seen_indices.insert(order_index, true);

        tracker
            .entry(order_index)
            .and_modify(|entry| {
                if entry.remaining > remaining + 1e-8 {
                    let fill = entry.remaining - remaining;
                    match entry.side {
                        QuoteSide::Bid => {
                            *inventory += fill;
                            hawkes_bid.on_event(now_secs);
                            log_action(&format!(
                                "🟣 acct-fill BID Δ={fill:.6} λb={:.2} | inv: {:.6} -> {:.6}",
                                hawkes_bid.value(now_secs),
                                *inventory - fill,  // Show before
                                *inventory          // Show after
                            ));
                            markouts.on_fill(
                                now_instant,
                                entry.price_ticks as f64 * tick_size,
                                QuoteSide::Bid,
                            );
                            breaker.on_fill(QuoteSide::Bid, now_instant);
                            telemetry.on_fill(now_instant, fill, QuoteSide::Bid);
                        }
                        QuoteSide::Ask => {
                            *inventory -= fill;
                            hawkes_ask.on_event(now_secs);
                            log_action(&format!(
                                "🟢 acct-fill ASK Δ={fill:.6} λa={:.2} | inv: {:.6} -> {:.6}",
                                hawkes_ask.value(now_secs),
                                *inventory + fill,  // Show before
                                *inventory          // Show after
                            ));
                            markouts.on_fill(
                                now_instant,
                                entry.price_ticks as f64 * tick_size,
                                QuoteSide::Ask,
                            );
                            breaker.on_fill(QuoteSide::Ask, now_instant);
                            telemetry.on_fill(now_instant, fill, QuoteSide::Ask);
                        }
                    }
                }
                entry.remaining = remaining;
                entry.price_ticks = price_ticks;
            })
            .or_insert_with(|| OrderTrackerEntry {
                remaining,
                side,
                price_ticks,
            });

        let state = SideOrderState {
            order_index,
            price_ticks,
            base_qty_units,
        };
        match side {
            QuoteSide::Bid => latest_bid = Some(state),
            QuoteSide::Ask => latest_ask = Some(state),
        }
    }

    let mut vanished = Vec::new();
    for (&order_index, entry) in tracker.iter() {
        if !seen_indices.contains_key(&order_index) && entry.remaining > 0.0 {
            vanished.push((order_index, entry.remaining, entry.side, entry.price_ticks));
        }
    }

    for (order_index, fill, side, price_ticks) in vanished {
        match side {
            QuoteSide::Bid => {
                *inventory += fill;
                hawkes_bid.on_event(now_secs);
                markouts.on_fill(now_instant, price_ticks as f64 * tick_size, QuoteSide::Bid);
                breaker.on_fill(QuoteSide::Bid, now_instant);
                telemetry.on_fill(now_instant, fill, QuoteSide::Bid);
            }
            QuoteSide::Ask => {
                *inventory -= fill;
                hawkes_ask.on_event(now_secs);
                markouts.on_fill(now_instant, price_ticks as f64 * tick_size, QuoteSide::Ask);
                breaker.on_fill(QuoteSide::Ask, now_instant);
                telemetry.on_fill(now_instant, fill, QuoteSide::Ask);
            }
        }
        tracker.remove(&order_index);
    }

    tracker.retain(|order_index, _| seen_indices.contains_key(order_index));

    active_orders.bid = latest_bid;
    active_orders.ask = latest_ask;

    if let Some(order) = &active_orders.bid {
        if pending.matches_recent(
            QuoteSide::Bid,
            order.price_ticks,
            order.base_qty_units,
            now_instant,
            PENDING_ORDER_TTL,
        ) {
            pending.clear(QuoteSide::Bid);
        }
    } else {
        pending.clear(QuoteSide::Bid);
    }

    if let Some(order) = &active_orders.ask {
        if pending.matches_recent(
            QuoteSide::Ask,
            order.price_ticks,
            order.base_qty_units,
            now_instant,
            PENDING_ORDER_TTL,
        ) {
            pending.clear(QuoteSide::Ask);
        }
    } else {
        pending.clear(QuoteSide::Ask);
    }
}

fn sync_inventory_from_positions(
    positions_obj: &serde_json::Map<String, serde_json::Value>,
    inventory: &mut f64,
) {
    let matched = positions_obj
        .get(&MARKET_ID.to_string())
        .or_else(|| {
            positions_obj.values().find(|val| {
                val.get("market_id")
                    .and_then(|v| v.as_i64())
                    .map(|id| id == MARKET_ID as i64)
                    .unwrap_or(false)
            })
        });

    let Some(pos_value) = matched else {
        return;
    };

    let position_raw = pos_value
        .get("position")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| v.as_f64())
        })
        .unwrap_or(0.0);

    let sign_hint = pos_value
        .get("sign")
        .and_then(|v| v.as_i64())
        .map(|s| s.clamp(-1, 1));

    let side_hint = pos_value
        .get("side")
        .and_then(|v| v.as_str())
        .map(|s| s.to_ascii_lowercase());

    let derived_sign = sign_hint.filter(|s| *s != 0).or_else(|| {
        side_hint.as_deref().map(|side| match side {
            "sell" | "short" => -1,
            "buy" | "long" => 1,
            _ => 0,
        })
    });

    let new_inventory = if let Some(sign) = derived_sign {
        position_raw.abs() * sign as f64
    } else {
        position_raw
    };

    if (new_inventory - *inventory).abs() > 1e-9 {
        log_action(&format!(
            "ℹ️  Synced inventory from positions feed: {:.6} (was {:.6})",
            new_inventory, *inventory
        ));
        log_action(&format!(
            "ℹ️  Position payload snapshot: position_raw={position_raw:.6}, sign_hint={:?}, side_hint={:?}",
            sign_hint, side_hint
        ));
        *inventory = new_inventory;
    }
}

fn process_trade_event(
    event: lighter_client::ws_client::TradeEvent,
    now_secs: f64,
    hawkes_bid: &mut HawkesSide,
    hawkes_ask: &mut HawkesSide,
) {
    let mut buys = 0usize;
    let mut sells = 0usize;
    for trade in event.trades {
        if trade.market_id != MARKET_ID as u32 {
            continue;
        }
        match trade.side.to_ascii_lowercase().as_str() {
            "buy" => {
                hawkes_ask.on_event(now_secs);
                buys += 1;
            }
            "sell" => {
                hawkes_bid.on_event(now_secs);
                sells += 1;
            }
            _ => {}
        }
    }
    if buys + sells > 0 {
        log_action(&format!("📈 Trade feed: buys={} sells={}", buys, sells));
    }
}

// --- Transaction event handling ------------------------------------------------------

fn handle_tx_event(event: WsEvent) {
    match event {
        WsEvent::Transaction(batch) => {
            if batch.txs.iter().any(|tx| tx.status != 1) {
                for tx in batch.txs {
                    log_action(&format!(
                        "🧾  Tx ack anomaly tx_type={} status={} hash={}",
                        tx.tx_type, tx.status, tx.hash
                    ));
                }
            }
        }
        WsEvent::Connected => log_action("Tx socket connected"),
        WsEvent::Pong => {}
        WsEvent::Closed(frame) => log_action(&format!("⚠️  Tx socket closed: {:?}", frame)),
        WsEvent::Unknown(_) => {
            // ignore verbose transport acks that the SDK does not model yet
        }
        other => {
            log_action(&format!("ℹ️  Tx event {:?}", other));
        }
    }
}

// --- Helpers -------------------------------------------------------------------------

async fn connect_order_book_stream(client: &LighterClient) -> Result<WsStream> {
    let stream = client
        .ws()
        .subscribe_order_book(MarketId::new(MARKET_ID))
        .connect()
        .await
        .context("Failed to connect order-book socket")?;
    log_action("Order-book socket connected");
    Ok(stream)
}

async fn connect_market_stats_stream(client: &LighterClient) -> Result<WsStream> {
    let stream = client
        .ws()
        .subscribe_market_stats(MarketId::new(MARKET_ID))
        .connect()
        .await
        .context("Failed to connect market-stats socket")?;
    log_action("Market-stats socket connected");
    Ok(stream)
}

async fn connect_account_orders_stream(
    client: &LighterClient,
    account_index: i64,
    auth_token: &str,
) -> Result<WsStream> {
    let mut stream = client
        .ws()
        .subscribe_account_all_orders(AccountId::new(account_index))
        .subscribe_account_all_positions(AccountId::new(account_index))
        .connect()
        .await
        .context("Failed to connect account socket")?;
    stream
        .connection_mut()
        .set_auth_token(auth_token.to_string());
    tokio::time::sleep(Duration::from_millis(50)).await;
    log_action("Account socket connected & authenticated");
    Ok(stream)
}

async fn connect_trade_stream(client: &LighterClient) -> Result<WsStream> {
    let stream = client
        .ws()
        .subscribe_trade(MarketId::new(MARKET_ID))
        .connect()
        .await
        .context("Failed to connect trade socket")?;
    log_action("Trade socket connected");
    Ok(stream)
}

async fn connect_transactions_stream(
    client: &LighterClient,
    auth_token: &str,
) -> Result<WsConnection> {
    let mut stream = client
        .ws()
        .subscribe_transactions()
        .connect()
        .await
        .context("Failed to connect transaction socket")?;
    stream
        .connection_mut()
        .set_auth_token(auth_token.to_string());
    tokio::time::sleep(Duration::from_millis(50)).await;
    log_action("Transaction socket connected & authenticated");
    Ok(stream.into_connection())
}

fn derive_bbo_guarded(
    now: Instant,
    book: &lighter_client::ws_client::OrderBookState,
    cfg: &TobGuardConfig,
    guard: &mut TobGuard,
) -> GuardOutcome {
    let bids: Vec<(f64, f64)> = book
        .bids
        .iter()
        .filter(|lvl| level_active(lvl))
        .filter_map(|lvl| safe_parse(&lvl.price).map(|price| (price, level_quantity(lvl))))
        .collect();

    let asks: Vec<(f64, f64)> = book
        .asks
        .iter()
        .filter(|lvl| level_active(lvl))
        .filter_map(|lvl| safe_parse(&lvl.price).map(|price| (price, level_quantity(lvl))))
        .collect();

    let Some(mut best_bid) = bids.first().map(|(p, _)| *p) else {
        return GuardOutcome::Escalate {
            reason: "no bids in book",
        };
    };
    let Some(mut best_ask) = asks.first().map(|(p, _)| *p) else {
        return GuardOutcome::Escalate {
            reason: "no asks in book",
        };
    };

    guard.on_frame_start(now, best_bid, best_ask);

    let ask_suspect = guard.ask_is_suspect(now, best_bid, best_ask, cfg);
    let bid_suspect = guard.bid_is_suspect(now, best_bid, best_ask, cfg);

    let mut used_skip = false;

    if ask_suspect {
        if let Some(clean) = asks
            .iter()
            .skip(1)
            .take(cfg.max_scan_levels)
            .filter(|(_, qty)| *qty > 0.0)
            .map(|(price, _)| *price)
            .find(|price| *price > best_bid + PRICE_EPS)
        {
            best_ask = clean;
            used_skip = true;
        }
    }

    if best_bid >= best_ask - PRICE_EPS && bid_suspect {
        if let Some(clean) = bids
            .iter()
            .skip(1)
            .take(cfg.max_scan_levels)
            .filter(|(_, qty)| *qty > 0.0)
            .map(|(price, _)| *price)
            .find(|price| *price < best_ask - PRICE_EPS)
        {
            best_bid = clean;
            used_skip = true;
        }
    }

    let crossed = best_ask <= best_bid + PRICE_EPS;
    guard.note_crossed(crossed);

    if crossed && guard.should_escalate(cfg) {
        return GuardOutcome::Escalate {
            reason: "Order book crossed after stale-filter",
        };
    }

    GuardOutcome::Bbo {
        bid: best_bid,
        ask: best_ask,
        skipped: used_skip,
    }
}

async fn submit_crash_trim_orders(
    client: &LighterClient,
    signer: &SignerClient,
    tx_conn: &mut WsConnection,
    inventory: f64,
    size_multiplier: i64,
    best_bid_ticks: i64,
    best_ask_ticks: i64,
) -> Result<bool> {
    let position = inventory;
    let abs_position = position.abs();
    if abs_position < CRASH_TRIM_MIN_QTY {
        return Ok(false);
    }

    let target_qty = (abs_position * CRASH_TRIM_RATIO)
        .max(CRASH_TRIM_MIN_QTY)
        .min(abs_position);
    let units = qty_to_units(target_qty, size_multiplier);
    if units <= 0 {
        return Ok(false);
    }

    let base_qty = qty_to_base(target_qty, size_multiplier)?;
    let (api_key, nonce) = signer.next_nonce().await?;

    let price_ticks = if position > 0.0 {
        best_bid_ticks.max(1)
    } else {
        best_ask_ticks.max(1)
    };

    let builder = if position > 0.0 {
        client.order(MarketId::new(MARKET_ID)).sell()
    } else {
        client.order(MarketId::new(MARKET_ID)).buy()
    }
    .qty(base_qty)
    .limit(Price::ticks(price_ticks))
    .ioc()
    .reduce_only();

    let payload = builder
        .with_api_key(ApiKeyIndex::new(api_key))
        .with_nonce(Nonce::new(nonce))
        .sign()
        .await
        .context("failed to sign crash trim order")?
        .payload()
        .to_string();

    let batch = vec![(TX_TYPE_CREATE_ORDER, payload)];
    send_batch_tx_ws(tx_conn, batch).await?;
    log_action(&format!(
        "⚠️  Crash trim IOC sent at {} ticks (inventory {:.6})",
        price_ticks, inventory
    ));
    Ok(true)
}

async fn fetch_rest_bbo(client: &LighterClient) -> Result<Option<(f64, f64)>> {
    let snapshot = client
        .orders()
        .book(MarketId::new(MARKET_ID), REST_SNAPSHOT_DEPTH)
        .await
        .context("REST order book snapshot failed")?;

    if snapshot.code != 200 {
        return Ok(None);
    }

    let mut best_bid: Option<f64> = None;
    for price in snapshot.bids.iter().filter_map(|order| {
        let qty = safe_parse(&order.remaining_base_amount)?;
        if qty <= 0.0 {
            return None;
        }
        safe_parse(&order.price)
    }) {
        best_bid = Some(best_bid.map_or(price, |p| p.max(price)));
    }

    let mut best_ask: Option<f64> = None;
    for price in snapshot.asks.iter().filter_map(|order| {
        let qty = safe_parse(&order.remaining_base_amount)?;
        if qty <= 0.0 {
            return None;
        }
        safe_parse(&order.price)
    }) {
        best_ask = Some(best_ask.map_or(price, |p| p.min(price)));
    }

    match (best_bid, best_ask) {
        (Some(bid), Some(ask)) if ask > bid + PRICE_EPS => Ok(Some((bid, ask))),
        _ => Ok(None),
    }
}

fn level_quantity(level: &OrderBookLevel) -> f64 {
    if let Some(remaining) = level
        .remaining_base_amount
        .as_deref()
        .and_then(|text| safe_parse(text))
    {
        remaining
    } else {
        safe_parse(&level.size).unwrap_or(0.0)
    }
}

fn level_active(level: &OrderBookLevel) -> bool {
    level_quantity(level) > 0.0
}

fn safe_parse(text: &str) -> Option<f64> {
    text.parse::<f64>().ok()
}

fn banner() {
    println!("══════════════════════════════════════════════════════════════");
    println!("  MM HAWKES DEMO (split sockets, Hawkes spreads)");
    println!("══════════════════════════════════════════════════════════════\n");
}

fn log_action(message: &str) {
    println!(
        "[{}] {}",
        Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        message
    );
}
