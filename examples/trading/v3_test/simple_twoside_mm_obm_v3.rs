//! Two-sided maker using split WebSocket connections (order book, market stats,
//! account orders, and transaction submission). Keeps the quoting logic very
//! simple: every refresh cancels everything and reposts a bid/ask pair around
//! the latest mark/mid, while the transport layer handles reconnection,
//! top-of-book guarding, and nonce recovery.

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    signer_client::SignerClient,
    tx_executor::{send_batch_tx_ws, TX_TYPE_CANCEL_ALL_ORDERS, TX_TYPE_CREATE_ORDER},
    types::{AccountId, ApiKeyIndex, BaseQty, MarketId, Nonce, Price},
    ws_client::{WsConnection, WsEvent, WsStream},
};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    time::Duration,
};
use tokio::time::{interval, sleep, Instant, MissedTickBehavior};

const MARKET_ID: i32 = 1;
const ORDER_SIZE: f64 = 0.0002;
const BASE_SPREAD_PCT: f64 = 0.00001;
const MAKER_FEE_PCT: f64 = 0.00002;
const TARGET_EDGE_BPS: f64 = 0.35;
const MARK_FRESH_FOR: Duration = Duration::from_millis(200);
const MIN_REFRESH_INTERVAL: Duration = Duration::from_millis(50);
const MAX_NO_MESSAGE_TIME: Duration = Duration::from_secs(15);

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
const NONCE_REPEAT_THRESHOLD: Duration = Duration::from_millis(300);

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

const DRY_RUN_ENV: &str = "SIMPLE_MM_DRY_RUN";

#[derive(Clone, Copy)]
enum ResubReason {
    Crossed,
    Unchanged,
    Stale,
}

impl ResubReason {
    fn as_str(self) -> &'static str {
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
        if self.last_crosses.len() <= 1 || self.jitter_mode_until.map_or(true, |until| now >= until)
        {
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

enum GuardOutcome {
    Bbo { bid: f64, ask: f64, skipped: bool },
    Escalate {
        reason: &'static str,
    },
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
            Some(_) => {
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
            None => {
                self.last_bid = Some(best_bid);
                self.last_bid_changed_at = Some(now);
                self.updates_since_bid_change = 0;
            }
        }

        match self.last_ask {
            Some(prev) if (best_ask - prev).abs() <= PRICE_EPS => {
                self.updates_since_ask_change = self.updates_since_ask_change.saturating_add(1);
            }
            Some(_) => {
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
            None => {
                self.last_ask = Some(best_ask);
                self.last_ask_changed_at = Some(now);
                self.updates_since_ask_change = 0;
            }
        }
    }

    fn ttl_from_ema(&self, ema_secs: Option<f64>, cfg: &TobGuardConfig) -> Duration {
        let ttl_secs = ema_secs
            .map(|ema| {
                (ema * cfg.ttl_multiplier)
                    .clamp(cfg.min_ttl.as_secs_f64(), cfg.max_ttl.as_secs_f64())
            })
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

struct Sockets {
    order: WsStream,
    stats: WsStream,
    account: WsStream,
    tx: WsConnection,
}

impl Sockets {
    async fn connect(
        client: &LighterClient,
        account_index: i64,
        auth_token: &str,
    ) -> Result<Self> {
        Ok(Self {
            order: connect_order_book_stream(client).await?,
            stats: connect_market_stats_stream(client).await?,
            account: connect_account_orders_stream(client, account_index, auth_token).await?,
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
        self.tx = connect_transactions_stream(client, auth_token).await?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    banner();

    let private_key =
        std::env::var("LIGHTER_PRIVATE_KEY").context("LIGHTER_PRIVATE_KEY not set")?;
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
    let base_qty = BaseQty::try_from((ORDER_SIZE * size_multiplier as f64).round() as i64)
        .map_err(|err| anyhow!("Invalid base quantity: {err}"))?;

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

    let mut generation = 0usize;

    loop {
        generation += 1;
        log_action(&format!("🔁 Streaming generation #{generation} started"));

        let mut last_order_message = Instant::now();
        let mut last_stats_message = Instant::now();
        let mut last_account_message = Instant::now();
        let mut last_tx_message = Instant::now();
        let mut last_refresh = Instant::now() - MIN_REFRESH_INTERVAL;
        let mut last_mark: Option<(f64, Instant)> = None;
        let mut market_watchdog = MarketWatchdog::new();
        let mut tob_guard = TobGuard::new();
        let mut last_rest_refresh: Option<Instant> = None;
        let mut nonce_refresh_times: HashMap<i32, Instant> = HashMap::new();
        let mut last_cancel_api_key: Option<i32> = None;
        let mut last_bid_api_key: Option<i32> = None;
        let mut last_ask_api_key: Option<i32> = None;

        let mut order_watchdog = interval(Duration::from_millis(200));
        order_watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut stats_watchdog = interval(Duration::from_millis(500));
        stats_watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut account_watchdog = interval(Duration::from_millis(500));
        account_watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut tx_watchdog = interval(Duration::from_millis(500));
        tx_watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);

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
                    if now.duration_since(last_account_message) > MAX_NO_MESSAGE_TIME {
                        log_action("⚠️  Account socket idle; reconnecting");
                        if let Err(err) = sockets.account.connection_mut().reconnect(Some(3)).await {
                            log_action(&format!("❌  Account reconnect failed: {err}"));
                            break 'event_loop;
                        }
                        last_account_message = Instant::now();
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
                order_evt = sockets.order.next() => {
                    match order_evt {
                        Some(Ok(WsEvent::OrderBook(ob))) => {
                            let now = Instant::now();
                            last_order_message = now;

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
                                log_action(&format!(
                                    "⚠️  Stale top level filtered; using deeper price bid={best_bid:.4} ask={best_ask:.4}"
                                ));
                            }

                            let best_bid_ticks = (best_bid / tick_size).floor() as i64;
                            let mut best_ask_ticks = (best_ask / tick_size).ceil() as i64;
                            if best_ask_ticks <= best_bid_ticks {
                                best_ask_ticks = best_bid_ticks + 1;
                            }

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

                            if best_ask_ticks <= best_bid_ticks {
                                log_action(&format!(
                                    "⚠️  Skipping crossed snapshot bid={best_bid:.4} ask={best_ask:.4}"
                                ));
                                continue;
                            }

                            if last_refresh.elapsed() < MIN_REFRESH_INTERVAL {
                                continue;
                            }

                            let mid = 0.5 * (best_bid + best_ask);
                            let mark = last_mark
                                .filter(|(_, ts)| ts.elapsed() <= MARK_FRESH_FOR)
                                .map(|(m, _)| m)
                                .unwrap_or(mid);

                            let spread_pct = (2.0 * MAKER_FEE_PCT + TARGET_EDGE_BPS * 0.0001)
                                .max(BASE_SPREAD_PCT);
                            let half_width = mark * spread_pct * 0.5;

                            let mut bid_price = (mark - half_width).max(tick_size);
                            let mut ask_price = mark + half_width;

                            let bid_ticks = (bid_price / tick_size).floor() as i64;
                            let mut ask_ticks = (ask_price / tick_size).ceil() as i64;
                            if ask_ticks <= bid_ticks {
                                ask_ticks = bid_ticks + 1;
                            }
                            bid_price = bid_ticks as f64 * tick_size;
                            ask_price = ask_ticks as f64 * tick_size;

                            log_action(&format!(
                                "mid={mid:.4} mark={mark:.4} -> bid={bid_price:.4} ask={ask_price:.4}"
                            ));

                            if dry_run {
                                log_action("🥽  DRY RUN: skipping cancel/bid/ask submission");
                                last_refresh = Instant::now();
                                continue;
                            }

                            let cancel_tx = {
                                let (api_key, nonce) = signer.next_nonce().await?;
                                last_cancel_api_key = Some(api_key);
                                signer
                                    .sign_cancel_all_orders(0, 0, Some(nonce), Some(api_key))
                                    .await
                                    .context("failed to sign cancel-all")?
                            };

                            let bid_tx = {
                                let (api_key, nonce) = signer.next_nonce().await?;
                                last_bid_api_key = Some(api_key);
                                client
                                    .order(MarketId::new(MARKET_ID))
                                    .buy()
                                    .qty(base_qty.clone())
                                    .limit(Price::ticks(bid_ticks))
                                    .post_only()
                                    .with_api_key(ApiKeyIndex::new(api_key))
                                    .with_nonce(Nonce::new(nonce))
                                    .sign()
                                    .await
                                    .context("failed to sign bid order")?
                                    .payload()
                                    .to_string()
                            };

                            let ask_tx = {
                                let (api_key, nonce) = signer.next_nonce().await?;
                                last_ask_api_key = Some(api_key);
                                client
                                    .order(MarketId::new(MARKET_ID))
                                    .sell()
                                    .qty(base_qty.clone())
                                    .limit(Price::ticks(ask_ticks))
                                    .post_only()
                                    .with_api_key(ApiKeyIndex::new(api_key))
                                    .with_nonce(Nonce::new(nonce))
                                    .sign()
                                    .await
                                    .context("failed to sign ask order")?
                                    .payload()
                                    .to_string()
                            };

                            let batch = vec![
                                (TX_TYPE_CANCEL_ALL_ORDERS, cancel_tx),
                                (TX_TYPE_CREATE_ORDER, bid_tx),
                                (TX_TYPE_CREATE_ORDER, ask_tx),
                            ];

                            let start = Instant::now();
                            match send_batch_tx_ws(&mut sockets.tx, batch).await {
                                Ok(results) => {
                                    log_action(&format!(
                                        "Batch result cancel={} bid={} ask={} ({:?})",
                                        results.get(0).copied().unwrap_or(false),
                                        results.get(1).copied().unwrap_or(false),
                                        results.get(2).copied().unwrap_or(false),
                                        start.elapsed()
                                    ));
                                    last_tx_message = Instant::now();
                                }
                                Err(err) => {
                                    log_action(&format!("❌  Batch submission failed: {err}; reconnecting tx socket"));
                                    if let Err(conn_err) = sockets.tx.reconnect(Some(3)).await {
                                        log_action(&format!("❌  Tx reconnect failed: {conn_err}"));
                                        break 'event_loop;
                                    }
                                    last_tx_message = Instant::now();
                                }
                            }

                            last_refresh = Instant::now();
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
                stats_evt = sockets.stats.next() => {
                    match stats_evt {
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
                account_evt = sockets.account.next() => {
                    match account_evt {
                        Some(Ok(WsEvent::Unknown(raw))) => {
                            last_account_message = Instant::now();
                            handle_account_payload(
                                &raw,
                                sockets.account.connection().suppressing_already_subscribed(),
                                &mut nonce_refresh_times,
                                &mut last_cancel_api_key,
                                &mut last_bid_api_key,
                                &mut last_ask_api_key,
                                &mut market_watchdog,
                                signer,
                            )
                            .await?;
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
                tx_evt = sockets.tx.next_event() => {
                    match tx_evt {
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
        sockets
            .rebuild_all(&client, account_index, &auth_token)
            .await?;
        market_watchdog.mark_recovered();
    }
}

fn handle_tx_event(event: WsEvent) {
    match event {
        WsEvent::Transaction(txs) => {
            if txs.txs.iter().any(|tx| tx.status != 1) {
                for tx in txs.txs {
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
        other => {
            log_action(&format!("ℹ️  Tx event {:?}", other));
        }
    }
}

async fn handle_account_payload(
    raw: &str,
    suppressing_already_subscribed: bool,
    nonce_refresh_times: &mut HashMap<i32, Instant>,
    last_cancel_api_key: &mut Option<i32>,
    last_bid_api_key: &mut Option<i32>,
    last_ask_api_key: &mut Option<i32>,
    market_watchdog: &mut MarketWatchdog,
    signer: &SignerClient,
) -> Result<()> {
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        if let Some(err) = value.get("error") {
            if err
                .get("code")
                .and_then(|c| c.as_i64())
                .map(|code| code == 30003)
                .unwrap_or(false)
                && suppressing_already_subscribed
            {
                return Ok(());
            }
            log_action(&format!("WS error: {}", err));
            if err
                .get("message")
                .and_then(|m| m.as_str())
                .map(|m| m.contains("invalid nonce"))
                .unwrap_or(false)
                || err
                    .get("code")
                    .and_then(|c| c.as_i64())
                    .map(|c| c == 21104)
                    .unwrap_or(false)
            {
                refresh_nonces(
                    signer,
                    nonce_refresh_times,
                    last_cancel_api_key,
                    last_bid_api_key,
                    last_ask_api_key,
                )
                .await?;
                market_watchdog.mark_recovered();
            }
        } else if let Some(code) = value.get("code") {
            if code.as_i64() == Some(30003) && suppressing_already_subscribed {
                return Ok(());
            }
            log_action(&format!("WS status: {} raw={}", code, truncate(raw, 200)));
            if code.as_i64() == Some(21104) {
                refresh_nonces(
                    signer,
                    nonce_refresh_times,
                    last_cancel_api_key,
                    last_bid_api_key,
                    last_ask_api_key,
                )
                .await?;
                market_watchdog.mark_recovered();
            }
        }
    } else {
        log_action(&format!("Account raw message: {}", truncate(raw, 200)));
    }
    Ok(())
}

async fn refresh_nonces(
    signer: &SignerClient,
    nonce_refresh_times: &mut HashMap<i32, Instant>,
    last_cancel_api_key: &mut Option<i32>,
    last_bid_api_key: &mut Option<i32>,
    last_ask_api_key: &mut Option<i32>,
) -> Result<()> {
    log_action("⚠️  Invalid nonce detected; refreshing signer state");
    let now = Instant::now();
    for api in [*last_cancel_api_key, *last_bid_api_key, *last_ask_api_key]
        .into_iter()
        .flatten()
    {
        if let Some(prev) = nonce_refresh_times.get(&api) {
            if now.duration_since(*prev) <= NONCE_REPEAT_THRESHOLD {
                if let Err(err) = signer.acknowledge_nonce_failure(api).await {
                    log_action(&format!(
                        "⚠️  Failed to acknowledge nonce failure for api_key {}: {err}",
                        api
                    ));
                } else {
                    log_action(&format!(
                        "⚠️  Acknowledged repeated nonce failure for api_key {}",
                        api
                    ));
                }
            }
        }
        match signer.refresh_nonce(api).await {
            Ok(()) => {
                nonce_refresh_times.insert(api, now);
            }
            Err(refresh_err) => {
                log_action(&format!(
                    "⚠️  Failed to refresh nonce for api_key {}: {refresh_err}",
                    api
                ));
            }
        }
    }
    *last_cancel_api_key = None;
    *last_bid_api_key = None;
    *last_ask_api_key = None;
    Ok(())
}

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

async fn connect_transactions_stream(
    client: &LighterClient,
    auth_token: &str,
) -> Result<WsConnection> {
    let mut stream = client
        .ws()
        .subscribe_transactions()
        .connect()
        .await
        .context("Failed to connect tx socket")?;
    stream
        .connection_mut()
        .set_auth_token(auth_token.to_string());
    tokio::time::sleep(Duration::from_millis(50)).await;
    log_action("Tx socket connected & authenticated");
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

fn level_quantity(level: &lighter_client::ws_client::OrderBookLevel) -> f64 {
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

fn level_active(level: &lighter_client::ws_client::OrderBookLevel) -> bool {
    level_quantity(level) > 0.0
}

fn safe_parse(text: &str) -> Option<f64> {
    text.parse::<f64>().ok()
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        format!("{}…", &value[..max])
    }
}

fn banner() {
    println!("══════════════════════════════════════════════════════════════");
    println!("  SIMPLE TWO-SIDED MM (v3 — multi-socket guard)");
    println!("══════════════════════════════════════════════════════════════\n");
}

fn log_action(message: &str) {
    println!(
        "[{}] {}",
        Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        message
    );
}
