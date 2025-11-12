use super::types::{
    FillEvent, FillSide, InventorySnapshot, QuoteOrder, QuotePair, StrategyDecision,
    StrategyMetrics,
};
use std::{
    collections::VecDeque,
    fs::{create_dir_all, OpenOptions},
    io::{BufWriter, Write},
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tracing::{debug, info, warn};

const MAX_MARKOUT_SAMPLES: usize = 200;
const SAME_SIDE_WINDOW: Duration = Duration::from_secs(1);
const FLIP_WINDOW: Duration = Duration::from_millis(200);
const MARKOUT_HORIZON: Duration = Duration::from_secs(1);
const INVENTORY_RESET_THRESHOLD: f64 = 0.05;
const MAX_FILL_RATE_SAMPLES: usize = 120;

#[derive(Debug, Clone)]
struct FillCsvRecord {
    timestamp_ms: i128,
    side: &'static str,
    price: f64,
    size: f64,
    inv_before: f64,
    inv_after: f64,
    mid_at_fill: f64,
    mid_plus_1s: f64,
    markout_bps: f64,
    same_side_fills_1s: usize,
    flips_200ms: usize,
    inventory_age_s: f64,
}

struct FillLogger {
    writer: Option<BufWriter<std::fs::File>>,
    path: PathBuf,
}

impl FillLogger {
    fn new(base: PathBuf) -> Self {
        Self {
            writer: None,
            path: base,
        }
    }

    fn ensure_writer(&mut self) -> std::io::Result<()> {
        if self.writer.is_none() {
            if let Some(parent) = self.path.parent() {
                create_dir_all(parent)?;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            let mut writer = BufWriter::new(file);
            writer.write_all(
                b"time_ms,side,price,size,inv_before,inv_after,mid_at_fill,mid_plus_1s,markout_bps,same_side_fills_1s,flips_200ms,inventory_age_s\n",
            )?;
            self.writer = Some(writer);
        }
        Ok(())
    }

    fn log(&mut self, record: &FillCsvRecord) {
        if self.ensure_writer().is_err() {
            return;
        }
        if let Some(writer) = self.writer.as_mut() {
            let line = format!(
                "{},{},{:.8},{:.8},{:.8},{:.8},{:.8},{:.8},{:.6},{},{},{}\n",
                record.timestamp_ms,
                record.side,
                record.price,
                record.size,
                record.inv_before,
                record.inv_after,
                record.mid_at_fill,
                record.mid_plus_1s,
                record.markout_bps,
                record.same_side_fills_1s,
                record.flips_200ms,
                record.inventory_age_s
            );
            let _ = writer.write_all(line.as_bytes());
        }
    }

    fn flush(&mut self) {
        if let Some(writer) = self.writer.as_mut() {
            let _ = writer.flush();
        }
    }
}

struct PendingFill {
    event: FillEvent,
    mid_at_fill: f64,
    inv_before: f64,
    inv_after: f64,
}

struct MarkoutTracker {
    pending: VecDeque<PendingFill>,
    markouts_bid: VecDeque<f64>,
    markouts_ask: VecDeque<f64>,
}

impl MarkoutTracker {
    fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            markouts_bid: VecDeque::new(),
            markouts_ask: VecDeque::new(),
        }
    }

    fn median(&self, side: FillSide) -> Option<f64> {
        match side {
            FillSide::Bid => median_from(&self.markouts_bid),
            FillSide::Ask => median_from(&self.markouts_ask),
        }
    }

    fn push_fill(&mut self, fill: PendingFill) {
        self.pending.push_back(fill);
    }

    fn update(&mut self, mid: f64, now: Instant) -> Vec<(FillSide, f64, PendingFill)> {
        let mut completed = Vec::new();
        while let Some(front) = self.pending.front() {
            if now.duration_since(front.event.timestamp) >= MARKOUT_HORIZON {
                let fill = self.pending.pop_front().unwrap();
                let markout = compute_markout(fill.event.side, fill.event.price, mid) * 10_000.0;
                match fill.event.side {
                    FillSide::Bid => {
                        push_capped(&mut self.markouts_bid, markout, MAX_MARKOUT_SAMPLES);
                    }
                    FillSide::Ask => {
                        push_capped(&mut self.markouts_ask, markout, MAX_MARKOUT_SAMPLES);
                    }
                }
                completed.push((fill.event.side, markout, fill));
            } else {
                break;
            }
        }
        completed
    }
}

fn compute_markout(side: FillSide, fill_price: f64, future_mid: f64) -> f64 {
    if fill_price <= 0.0 {
        return 0.0;
    }
    let diff = future_mid - fill_price;
    match side {
        FillSide::Bid => diff / fill_price,
        FillSide::Ask => -diff / fill_price,
    }
}

fn push_capped<T>(deque: &mut VecDeque<T>, value: T, max: usize) {
    if deque.len() >= max {
        deque.pop_front();
    }
    deque.push_back(value);
}

fn median_from(data: &VecDeque<f64>) -> Option<f64> {
    if data.is_empty() {
        return None;
    }
    let mut vec = data.iter().copied().collect::<Vec<_>>();
    vec.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = vec.len() / 2;
    if vec.len() % 2 == 0 {
        Some((vec[mid - 1] + vec[mid]) / 2.0)
    } else {
        Some(vec[mid])
    }
}

#[derive(Clone, Copy)]
pub struct LadderLevel {
    pub ratio: f64,
    pub offset_bps: f64,
    pub size_multiplier: f64,
}

fn default_ladder_levels() -> Vec<LadderLevel> {
    vec![
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
    ]
}

pub struct ParticipationConfig {
    pub enable_logging: bool,
    pub fill_log_path: Option<PathBuf>,
    pub base_pause_ms: u64,
    pub toxicity_pause_ms: u64,
    pub same_side_limit_per_sec: usize,
    pub kill_switch_markout_bps: f64,
    pub kill_pause_secs: u64,
    pub global_pnl_threshold_bps_per_mm: f64,
    pub global_pause_secs: u64,
    pub flip_gate_threshold: usize,
    pub flip_pause_ms: u64,
    pub inventory_age_threshold_s: u64,
    pub inventory_partial_bps: f64,
    pub inventory_partial_fraction: f64,
    pub inventory_partial_cooldown_ms: u64,
    pub flip_widen_bps: f64,
    pub flip_widen_duration_ms: u64,
    pub ladder_hysteresis_ratio: f64,
    pub base_order_size: f64,
    pub ladder: Vec<LadderLevel>,
}

impl Default for ParticipationConfig {
    fn default() -> Self {
        Self {
            enable_logging: true,
            fill_log_path: Some(PathBuf::from("logs/fills/session.csv")),
            base_pause_ms: 350,
            toxicity_pause_ms: 900,
            same_side_limit_per_sec: 3,
            kill_switch_markout_bps: -0.10,
            kill_pause_secs: 5,
            global_pnl_threshold_bps_per_mm: -0.30,
            global_pause_secs: 10,
            flip_gate_threshold: 8,
            flip_pause_ms: 600,
            flip_widen_bps: 1.0,
            flip_widen_duration_ms: 1_200,
            inventory_age_threshold_s: 8,
            inventory_partial_bps: 6.0,
            inventory_partial_fraction: 0.33,
            inventory_partial_cooldown_ms: 2_000,
            ladder_hysteresis_ratio: 0.05,
            base_order_size: 0.0,
            ladder: default_ladder_levels(),
        }
    }
}

struct SideState {
    paused_until: Option<Instant>,
    kill_until: Option<Instant>,
    fills_last_sec: VecDeque<Instant>,
    current_bucket_start: Option<Instant>,
    current_bucket_count: u32,
    fills_per_second: VecDeque<u32>,
}

impl SideState {
    fn new() -> Self {
        Self {
            paused_until: None,
            kill_until: None,
            fills_last_sec: VecDeque::new(),
            current_bucket_start: None,
            current_bucket_count: 0,
            fills_per_second: VecDeque::new(),
        }
    }

    fn record_fill(&mut self, timestamp: Instant) {
        self.fills_last_sec.push_back(timestamp);
        while let Some(front) = self.fills_last_sec.front() {
            if timestamp.duration_since(*front) > SAME_SIDE_WINDOW {
                self.fills_last_sec.pop_front();
            } else {
                break;
            }
        }
        self.update_per_second(timestamp);
    }

    fn enforce_pause(&mut self, until: Instant) {
        match self.paused_until {
            Some(orig) if orig > until => {}
            _ => self.paused_until = Some(until),
        }
    }

    fn enforce_kill(&mut self, until: Instant) {
        match self.kill_until {
            Some(orig) if orig > until => {}
            _ => self.kill_until = Some(until),
        }
    }

    fn is_blocked(&self, now: Instant) -> bool {
        self.kill_until.map(|t| now < t).unwrap_or(false)
            || self.paused_until.map(|t| now < t).unwrap_or(false)
    }

    fn is_kill_active(&self, now: Instant) -> bool {
        self.kill_until.map(|t| now < t).unwrap_or(false)
    }

    fn fills_per_second_percentile(&self, percentile: f64) -> f64 {
        let mut data: Vec<u32> = self.fills_per_second.iter().copied().collect();
        if self.current_bucket_count > 0 {
            data.push(self.current_bucket_count);
        }
        if data.is_empty() {
            return 0.0;
        }
        data.sort_unstable();
        let clamped = percentile.clamp(0.0, 1.0);
        let idx = ((data.len() - 1) as f64 * clamped).round() as usize;
        data[idx] as f64
    }

    fn update_per_second(&mut self, timestamp: Instant) {
        match self.current_bucket_start {
            Some(start) => {
                if timestamp.duration_since(start) >= Duration::from_secs(1) {
                    self.push_bucket();
                    self.current_bucket_start = Some(timestamp);
                    self.current_bucket_count = 1;
                } else {
                    self.current_bucket_count += 1;
                }
            }
            None => {
                self.current_bucket_start = Some(timestamp);
                self.current_bucket_count = 1;
            }
        }
    }

    fn push_bucket(&mut self) {
        if self.current_bucket_count > 0 {
            if self.fills_per_second.len() >= MAX_FILL_RATE_SAMPLES {
                self.fills_per_second.pop_front();
            }
            self.fills_per_second.push_back(self.current_bucket_count);
        }
        self.current_bucket_count = 0;
    }
}

struct GlobalPnlTracker {
    window: VecDeque<(Instant, f64, f64)>,
    sum_notional: f64,
    sum_pnl: f64,
    last_net_pnl: f64,
    horizon: Duration,
}

impl GlobalPnlTracker {
    fn new(horizon: Duration) -> Self {
        Self {
            window: VecDeque::new(),
            sum_notional: 0.0,
            sum_pnl: 0.0,
            last_net_pnl: 0.0,
            horizon,
        }
    }

    fn record(&mut self, now: Instant, notional: f64, net_pnl: f64) {
        let delta_pnl = net_pnl - self.last_net_pnl;
        self.last_net_pnl = net_pnl;
        self.window.push_back((now, notional, delta_pnl));
        self.sum_notional += notional;
        self.sum_pnl += delta_pnl;
        self.evict(now);
    }

    fn evict(&mut self, now: Instant) {
        while let Some(front) = self.window.front() {
            if now.duration_since(front.0) > self.horizon {
                let (_, notional, pnl) = self.window.pop_front().unwrap();
                self.sum_notional -= notional;
                self.sum_pnl -= pnl;
            } else {
                break;
            }
        }
    }

    fn pnl_per_million(&self) -> Option<f64> {
        if self.sum_notional <= 0.0 {
            return None;
        }
        Some(self.sum_pnl / (self.sum_notional / 1_000_000.0))
    }
}

#[derive(Clone, Debug, Default)]
pub struct ParticipationMetricsSnapshot {
    pub median_markout_bid_bps: Option<f64>,
    pub median_markout_ask_bps: Option<f64>,
    pub same_side_fills_bid: usize,
    pub same_side_fills_ask: usize,
    pub same_side_fills_p95_bid: f64,
    pub same_side_fills_p95_ask: f64,
    pub flip_rate_hz: f64,
    pub flips_200ms: usize,
    pub inventory_age_s: f64,
    pub pnl_per_million: Option<f64>,
    pub global_pause_active: bool,
    pub kill_active_bid: bool,
    pub kill_active_ask: bool,
}

pub struct ParticipationController {
    config: ParticipationConfig,
    fill_logger: Option<FillLogger>,
    markouts: MarkoutTracker,
    side_state: [SideState; 2],
    flips: VecDeque<Instant>,
    last_mid: Option<f64>,
    last_mid_direction: Option<i8>,
    inventory_age_start: Instant,
    last_inventory_snapshot: InventorySnapshot,
    global_kill_until: Option<Instant>,
    pnl_tracker: GlobalPnlTracker,
    inventory_entry_mid: Option<f64>,
    last_partial_exit: Option<Instant>,
    flip_widen_until: Option<Instant>,
    active_ladder_index: Option<usize>,
}

impl ParticipationController {
    pub fn new(config: ParticipationConfig, snapshot: InventorySnapshot) -> Self {
        let log_path = config.fill_log_path.clone();
        Self {
            fill_logger: log_path.map(FillLogger::new),
            config,
            markouts: MarkoutTracker::new(),
            side_state: [SideState::new(), SideState::new()],
            flips: VecDeque::new(),
            last_mid: None,
            last_mid_direction: None,
            inventory_age_start: Instant::now(),
            last_inventory_snapshot: snapshot.clone(),
            global_kill_until: None,
            pnl_tracker: GlobalPnlTracker::new(Duration::from_secs(3600)),
            inventory_entry_mid: Some(snapshot.mid_price),
            last_partial_exit: None,
            flip_widen_until: None,
            active_ladder_index: None,
        }
    }

    pub fn on_market_tick(&mut self, mid: f64, now: Instant) {
        if let Some(prev_mid) = self.last_mid {
            let direction = if (mid - prev_mid).abs() < f64::EPSILON {
                0
            } else if mid > prev_mid {
                1
            } else {
                -1
            };
            if direction != 0 {
                if let Some(last_dir) = self.last_mid_direction {
                    if direction != last_dir {
                        self.flips.push_back(now);
                    }
                }
                self.last_mid_direction = Some(direction);
            }
            while let Some(front) = self.flips.front() {
                if now.duration_since(*front) > FLIP_WINDOW {
                    self.flips.pop_front();
                } else {
                    break;
                }
            }
        }
        self.last_mid = Some(mid);

        let completed = self.markouts.update(mid, now);
        for (side, markout, fill) in completed {
            let side_idx = match side {
                FillSide::Bid => 0,
                FillSide::Ask => 1,
            };
            if let Some(logger) = self.fill_logger.as_mut() {
                let record = FillCsvRecord {
                    timestamp_ms: unix_millis_now(),
                    side: match side {
                        FillSide::Bid => "bid",
                        FillSide::Ask => "ask",
                    },
                    price: fill.event.price,
                    size: fill.event.size,
                    inv_before: fill.inv_before,
                    inv_after: fill.inv_after,
                    mid_at_fill: fill.mid_at_fill,
                    mid_plus_1s: mid,
                    markout_bps: markout,
                    same_side_fills_1s: self.side_state[side_idx].fills_last_sec.len(),
                    flips_200ms: self.flips.len(),
                    inventory_age_s: now.duration_since(self.inventory_age_start).as_secs_f64(),
                };
                logger.log(&record);
            }

            if markout <= self.config.kill_switch_markout_bps {
                let until = now + Duration::from_secs(self.config.kill_pause_secs);
                self.side_state[side_idx].enforce_kill(until);
            } else if markout < 0.0 {
                // Extend pause for mild toxicity.
                let extra =
                    now + Duration::from_millis(self.config.toxicity_pause_ms.saturating_sub(100));
                self.side_state[side_idx].enforce_pause(extra);
            }
        }
    }

    pub fn on_fill(
        &mut self,
        fill: &FillEvent,
        mid: f64,
        inventory_before: f64,
        inventory_after: f64,
        metrics: &StrategyMetrics,
    ) {
        let side_idx = match fill.side {
            FillSide::Bid => 0,
            FillSide::Ask => 1,
        };
        let now = fill.timestamp;

        self.side_state[side_idx].record_fill(now);

        let pause_until = now + Duration::from_millis(self.config.base_pause_ms);
        self.side_state[side_idx].enforce_pause(pause_until);

        if self.side_state[side_idx].fills_last_sec.len() > self.config.same_side_limit_per_sec {
            let until = now + Duration::from_millis(self.config.toxicity_pause_ms);
            self.side_state[side_idx].enforce_pause(until);
        }

        let p95 = self.side_state[side_idx].fills_per_second_percentile(0.95);
        if p95 > self.config.same_side_limit_per_sec as f64 {
            let until = now + Duration::from_millis(self.config.toxicity_pause_ms);
            self.side_state[side_idx].enforce_pause(until);
            debug!(
                "Same-side fill rate p95 {:.1} exceeded limit; pausing {:?}",
                p95, self.config.toxicity_pause_ms
            );
        }

        let pending = PendingFill {
            event: fill.clone(),
            mid_at_fill: mid,
            inv_before: inventory_before,
            inv_after: inventory_after,
        };
        self.markouts.push_fill(pending);

        let notional = fill.price.abs() * fill.size.abs();
        self.pnl_tracker.record(now, notional, metrics.net_pnl);

        if let Some(pnl_per_mm) = self.pnl_tracker.pnl_per_million() {
            if pnl_per_mm < self.config.global_pnl_threshold_bps_per_mm {
                self.global_kill_until =
                    Some(now + Duration::from_secs(self.config.global_pause_secs));
            }
        }
    }

    pub fn update_inventory_snapshot(&mut self, snapshot: &InventorySnapshot, now: Instant) {
        self.last_inventory_snapshot = snapshot.clone();
        if snapshot.normalized_inventory.abs() <= INVENTORY_RESET_THRESHOLD {
            self.inventory_age_start = now;
            self.inventory_entry_mid = Some(snapshot.mid_price);
            self.last_partial_exit = None;
        }
    }

    fn handle_flip_gate(&mut self, decision: &mut QuotePair, mid: f64, now: Instant) {
        let until = now + Duration::from_millis(self.config.flip_pause_ms);
        self.side_state[0].enforce_pause(until);
        self.side_state[1].enforce_pause(until);
        self.flips.clear();
        if self.config.flip_widen_bps > 0.0 {
            let widen = mid * (self.config.flip_widen_bps / 10_000.0);
            decision.bid.price -= widen;
            decision.ask.price += widen;
            let widen_duration = Duration::from_millis(
                self.config
                    .flip_widen_duration_ms
                    .max(self.config.flip_pause_ms),
            );
            self.flip_widen_until = Some(now + widen_duration);
            info!(
                "Flip gate triggered: widening quotes by {:.2} bps for {:?}",
                self.config.flip_widen_bps, widen_duration
            );
        } else {
            self.flip_widen_until = None;
            info!("Flip gate triggered: pausing sides without widening");
        }
    }

    fn try_partial_exit(
        &mut self,
        snapshot: &InventorySnapshot,
        mid: f64,
        now: Instant,
    ) -> Option<StrategyDecision> {
        let threshold_s = self.config.inventory_age_threshold_s;
        if threshold_s == 0 {
            return None;
        }
        let age = now
            .checked_duration_since(self.inventory_age_start)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if age < threshold_s {
            return None;
        }
        let heavy_side = snapshot.base_balance;
        if heavy_side.abs() <= f64::EPSILON {
            return None;
        }
        let entry_mid = self.inventory_entry_mid.unwrap_or(mid);
        if entry_mid <= 0.0 || mid <= 0.0 {
            return None;
        }
        let price_change = (mid - entry_mid) / entry_mid;
        let threshold_ratio = self.config.inventory_partial_bps / 10_000.0;
        let favorable = if heavy_side > 0.0 {
            price_change >= threshold_ratio
        } else {
            price_change <= -threshold_ratio
        };
        if !favorable {
            return None;
        }
        let diff_bps = price_change * 10_000.0;
        if diff_bps.abs() < self.config.inventory_partial_bps {
            return None;
        }
        if let Some(last_exit) = self.last_partial_exit {
            if now.duration_since(last_exit).as_millis()
                < self.config.inventory_partial_cooldown_ms as u128
            {
                return None;
            }
        }
        if self.config.base_order_size <= 0.0 {
            return None;
        }
        let fraction = self.config.inventory_partial_fraction.clamp(0.0, 1.0);
        let desired = (self.config.base_order_size * fraction).max(1e-9);
        let size = heavy_side.abs().min(desired);
        if size <= 1e-9 {
            return None;
        }
        let mut order = QuoteOrder::new(mid, size, "inventory_partial");
        self.last_partial_exit = Some(now);
        if heavy_side > 0.0 {
            order.label = "inventory_partial_ask";
            info!(
                "Inventory partial exit triggered (ask) | age: {}s | diff: {:.2} bps | size: {:.6}",
                age, diff_bps, size
            );
            Some(StrategyDecision::QuoteAskOnly(order))
        } else {
            order.label = "inventory_partial_bid";
            info!(
                "Inventory partial exit triggered (bid) | age: {}s | diff: {:.2} bps | size: {:.6}",
                age, diff_bps, size
            );
            Some(StrategyDecision::QuoteBidOnly(order))
        }
    }

    pub fn apply_participation_rules(
        &mut self,
        mut decision: QuotePair,
        now: Instant,
    ) -> Option<StrategyDecision> {
        if self
            .global_kill_until
            .map(|until| now < until)
            .unwrap_or(false)
        {
            return Some(StrategyDecision::Skip("global_pause"));
        }

        let snapshot = self.last_inventory_snapshot.clone();
        let mid = decision.reservation_price;

        if self.config.flip_gate_threshold > 0
            && self.flips.len() >= self.config.flip_gate_threshold
        {
            self.handle_flip_gate(&mut decision, mid, now);
        } else if let Some(until) = self.flip_widen_until {
            if now >= until {
                self.flip_widen_until = None;
            } else if self.config.flip_widen_bps > 0.0 {
                let widen = mid * (self.config.flip_widen_bps / 10_000.0);
                decision.bid.price -= widen;
                decision.ask.price += widen;
            }
        }

        if let Some(partial) = self.try_partial_exit(&snapshot, mid, now) {
            return Some(partial);
        }

        let normalized = snapshot.normalized_inventory.abs();
        let mut target_index: Option<usize> = None;
        for (idx, level) in self.config.ladder.iter().enumerate() {
            if normalized >= level.ratio {
                target_index = Some(idx);
            }
        }
        if let Some(active) = self.active_ladder_index {
            if let Some(level) = self.config.ladder.get(active) {
                let release_threshold =
                    (level.ratio - self.config.ladder_hysteresis_ratio).max(0.0);
                if normalized >= release_threshold {
                    target_index = Some(active);
                }
            }
        }
        self.active_ladder_index = target_index;

        if let Some(idx) = self.active_ladder_index {
            if let Some(level) = self.config.ladder.get(idx) {
                if snapshot.normalized_inventory > 0.0 {
                    decision.ask.price += mid * (level.offset_bps / 10_000.0);
                    decision.ask.size *= level.size_multiplier;
                } else if snapshot.normalized_inventory < 0.0 {
                    decision.bid.price -= mid * (level.offset_bps / 10_000.0);
                    decision.bid.size *= level.size_multiplier;
                }
            }
        }

        let mut bid_allowed = !self.side_state[0].is_blocked(now);
        let mut ask_allowed = !self.side_state[1].is_blocked(now);
        if decision.bid.size < 1e-12 {
            bid_allowed = false;
        }
        if decision.ask.size < 1e-12 {
            ask_allowed = false;
        }

        match (bid_allowed, ask_allowed) {
            (true, true) => Some(StrategyDecision::Quote(decision)),
            (true, false) => Some(StrategyDecision::QuoteBidOnly(decision.bid)),
            (false, true) => Some(StrategyDecision::QuoteAskOnly(decision.ask)),
            (false, false) => Some(StrategyDecision::Skip("side_pause")),
        }
    }

    pub fn finalize(&mut self) {
        if let Some(logger) = self.fill_logger.as_mut() {
            logger.flush();
        }
    }

    pub fn metrics_snapshot(&self) -> ParticipationMetricsSnapshot {
        let now = Instant::now();
        let flips_rate = self.flips.len() as f64 / FLIP_WINDOW.as_secs_f64();
        let p95_bid = self.side_state[0].fills_per_second_percentile(0.95);
        let p95_ask = self.side_state[1].fills_per_second_percentile(0.95);
        ParticipationMetricsSnapshot {
            median_markout_bid_bps: self.markouts.median(FillSide::Bid),
            median_markout_ask_bps: self.markouts.median(FillSide::Ask),
            same_side_fills_bid: self.side_state[0].fills_last_sec.len(),
            same_side_fills_ask: self.side_state[1].fills_last_sec.len(),
            same_side_fills_p95_bid: p95_bid,
            same_side_fills_p95_ask: p95_ask,
            flip_rate_hz: flips_rate,
            flips_200ms: self.flips.len(),
            inventory_age_s: now.duration_since(self.inventory_age_start).as_secs_f64(),
            pnl_per_million: self.pnl_tracker.pnl_per_million(),
            global_pause_active: self
                .global_kill_until
                .map(|until| now < until)
                .unwrap_or(false),
            kill_active_bid: self.side_state[0].is_kill_active(now),
            kill_active_ask: self.side_state[1].is_kill_active(now),
        }
    }
}

fn unix_millis_now() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i128)
        .unwrap_or_default()
}
