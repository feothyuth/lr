//! Extremely simple two-sided maker: every order-book update cancels then reposts
//! symmetric post-only quotes around the live mid (or freshest mark). All of the
//! protective guardrails from the v1 bot—clamping, dwell/hop thresholds, inventory
//! skew, post-only slack, etc.—have been removed on purpose so you can observe the
//! raw behaviour against the venue feed.
//!
//! Environment variables:
//! - `LIGHTER_PRIVATE_KEY`
//! - `LIGHTER_ACCOUNT_INDEX` (or `ACCOUNT_INDEX`)
//! - `LIGHTER_API_KEY_INDEX`
//! - Optional: `LIGHTER_API_URL`

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    tx_executor::{send_batch_tx_ws, TX_TYPE_CANCEL_ALL_ORDERS, TX_TYPE_CREATE_ORDER},
    types::{AccountId, ApiKeyIndex, BaseQty, MarketId, Nonce, Price},
    ws_client::WsEvent,
};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::time::Duration;
use tokio::time::{interval, sleep, Instant};

const MARKET_ID: i32 = 1;
const ORDER_SIZE: f64 = 0.0002;
const BASE_SPREAD_PCT: f64 = 0.00001;
const MAKER_FEE_PCT: f64 = 0.00002;
const TARGET_EDGE_BPS: f64 = 0.35;
const MARK_FRESH_FOR: Duration = Duration::from_millis(200);
const MIN_REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const MAX_NO_MESSAGE_TIME: Duration = Duration::from_secs(15);
const MAX_CONNECTION_AGE: Duration = Duration::from_secs(300);

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

    fn is_paused(&self) -> bool {
        self.pending
    }
}

const STALE_TTL: Duration = Duration::from_millis(600);
const MIN_UPDATES_BEFORE_SUSPECT: u32 = 10;
const MAX_LEVELS_TO_SCAN: usize = 8;
const MAX_CONSEC_CROSSED_BEFORE_ESCALATE: u32 = 6;
const PRICE_EPS: f64 = 1e-9;

#[derive(Default)]
struct TobGuard {
    last_bid: Option<f64>,
    last_ask: Option<f64>,
    last_bid_changed_at: Option<Instant>,
    last_ask_changed_at: Option<Instant>,
    updates_since_bid_change: u32,
    updates_since_ask_change: u32,
    crossed_frames: u32,
}

impl TobGuard {
    fn on_frame_start(&mut self, now: Instant, best_bid: f64, best_ask: f64) {
        match self.last_bid {
            Some(prev) if (best_bid - prev).abs() <= PRICE_EPS => {
                self.updates_since_bid_change = self.updates_since_bid_change.saturating_add(1);
            }
            _ => {
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
                self.last_ask = Some(best_ask);
                self.last_ask_changed_at = Some(now);
                self.updates_since_ask_change = 0;
            }
        }
    }

    fn ask_is_suspect(&self, now: Instant, best_bid: f64, best_ask: f64) -> bool {
        best_ask <= best_bid + PRICE_EPS
            && self
                .last_ask_changed_at
                .map(|ts| now.duration_since(ts) >= STALE_TTL)
                .unwrap_or(false)
            && self.updates_since_ask_change >= MIN_UPDATES_BEFORE_SUSPECT
    }

    fn bid_is_suspect(&self, now: Instant, best_bid: f64, best_ask: f64) -> bool {
        best_bid >= best_ask - PRICE_EPS
            && self
                .last_bid_changed_at
                .map(|ts| now.duration_since(ts) >= STALE_TTL)
                .unwrap_or(false)
            && self.updates_since_bid_change >= MIN_UPDATES_BEFORE_SUSPECT
    }

    fn note_crossed(&mut self, crossed: bool) {
        if crossed {
            self.crossed_frames = self.crossed_frames.saturating_add(1);
        } else {
            self.crossed_frames = 0;
        }
    }

    fn should_escalate(&self) -> bool {
        self.crossed_frames >= MAX_CONSEC_CROSSED_BEFORE_ESCALATE
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    banner();

    // --- Environment / client bootstrap -------------------------------------------------------
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

    // --- WebSocket connection -----------------------------------------------------------------
    let auth_token = client.create_auth_token(None)?;
    let mut stream = connect_stream(&client, &auth_token, account_index).await?;

    let mut reconnects = 0usize;

    loop {
        if reconnects > 0 {
            log_action(&format!("Reconnecting (attempt #{reconnects})"));
            stream = connect_stream(&client, &auth_token, account_index).await?;
        }

        let mut last_any_message = Instant::now();
        let connection_start = Instant::now();
        let mut last_refresh = Instant::now() - MIN_REFRESH_INTERVAL;
        let mut last_mark: Option<(f64, Instant)> = None;
        let mut watchdog_timer = interval(Duration::from_secs(5));
        let mut market_watchdog = MarketWatchdog::new();
        let mut tob_guard = TobGuard::default();
        let mut last_cancel_api_key: Option<i32> = None;
        let mut last_bid_api_key: Option<i32> = None;
        let mut last_ask_api_key: Option<i32> = None;
        let mut nonce_refresh_times: HashMap<i32, Instant> = HashMap::new();
        let mut reconnect_start: Option<Instant> = None;
        let mut reconnect_connected: Option<Instant> = None;

        loop {
            tokio::select! {
                _ = watchdog_timer.tick() => {
                    let now = Instant::now();
                    if now.duration_since(last_any_message) > MAX_NO_MESSAGE_TIME {
                        log_action("⚠️  WebSocket silent too long; reconnecting");
                        reconnects += 1;
                        break;
                    }
                    if now.duration_since(connection_start) > MAX_CONNECTION_AGE {
                        log_action("WebSocket connection aged out; reconnecting");
                        reconnects += 1;
                        break;
                    }
                }
                maybe_event = stream.next() => {
                    let Some(event) = maybe_event else {
                        log_action("WebSocket stream closed by server");
                        reconnects += 1;
                        break;
                    };

                    match event {
                        Ok(WsEvent::OrderBook(ob)) => {
                            let now = Instant::now();
                            last_any_message = now;

                            let (best_bid, best_ask, skipped_level) =
                                match derive_bbo_guarded(now, &ob.state, &mut tob_guard) {
                                    Some(values) => values,
                                    None => {
                                        log_action("⚠️  Order book remains crossed after guard; forcing reconnect");
                                        reconnects += 1;
                                        break;
                                    }
                                };

                            if skipped_level {
                                log_action(&format!(
                                    "⚠️  Ignored stale top level; using deeper price bid={best_bid:.4} ask={best_ask:.4}"
                                ));
                            }

                            let best_bid_ticks = (best_bid / tick_size).round() as i64;
                            let best_ask_ticks = (best_ask / tick_size).round() as i64;

                            if market_watchdog.is_paused() {
                                if best_ask > best_bid {
                                    if let Some(start) = reconnect_start {
                                        log_action(&format!(
                                            "    first clean frame after {:?}",
                                            start.elapsed()
                                        ));
                                    }
                                    market_watchdog.mark_recovered();
                                } else {
                                    continue;
                                }
                            }

                            match market_watchdog.observe(best_bid_ticks, best_ask_ticks, now) {
                                WatchdogDecision::None => {}
                                WatchdogDecision::Reconnect { delay, reason, attempts, escalate } => {
                                    log_action(&format!(
                                        "⚠️  Watchdog reconnecting order_book/{MARKET_ID} (reason={}, attempts_last_min={attempts})",
                                        reason.as_str()
                                    ));
                                    if escalate {
                                        log_action("⚠️  Watchdog escalation: repeated reconnect attempts");
                                    }
                                    let fast_resubscribe = !escalate && attempts == 1 && delay.is_zero();
                                    if fast_resubscribe {
                                        log_action("Attempting in-place order book resubscribe (no socket restart)");
                                        if let Err(err) = stream
                                            .connection_mut()
                                            .resubscribe_order_book_channel(MarketId::new(MARKET_ID))
                                            .await
                                        {
                                            log_action(&format!("⚠️  Resubscribe failed ({err}); falling back to reconnect"));
                                        } else {
                                            last_any_message = Instant::now();
                                            reconnect_start = Some(Instant::now());
                                            reconnect_connected = Some(Instant::now());
                                            continue;
                                        }
                                    }
                                    reconnect_start = Some(Instant::now());
                                    reconnect_connected = None;
                                    if !delay.is_zero() {
                                        sleep(delay).await;
                                    }
                                    if let Err(err) = stream.connection_mut().reconnect(Some(3)).await {
                                        log_action(&format!("❌  Watchdog reconnect failed: {err}"));
                                        reconnects += 1;
                                        break;
                                    }
                                    last_any_message = Instant::now();
                                    continue;
                                }
                            }

                            if market_watchdog.is_paused() {
                                continue;
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

                            // Map to ticks with minimal adjustment to stay separated.
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

                            if let Some(start) = reconnect_start {
                                if let Some(socket_elapsed) =
                                    reconnect_connected.map(|connected| connected.duration_since(start))
                                {
                                    log_action(&format!("    socket ready after {:?}", socket_elapsed));
                                }
                                log_action(&format!("✅  Recovered quoting in {:?}", start.elapsed()));
                                reconnect_start = None;
                                reconnect_connected = None;
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
                            let results = send_batch_tx_ws(stream.connection_mut(), batch.clone())
                                .await
                                .context("batch submission failed")?;

                            log_action(&format!(
                                "Batch result cancel={} bid={} ask={} ({:?})",
                                results.get(0).copied().unwrap_or(false),
                                results.get(1).copied().unwrap_or(false),
                                results.get(2).copied().unwrap_or(false),
                                start.elapsed()
                            ));

                            last_refresh = Instant::now();
                        }
                        Ok(WsEvent::MarketStats(stats)) => {
                            last_any_message = Instant::now();
                            if let Ok(mark) = parse_price(&stats.market_stats.mark_price) {
                                last_mark = Some((mark, Instant::now()));
                            }
                        }
                        Ok(WsEvent::Unknown(raw)) => {
                            last_any_message = Instant::now();
                            if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                                if let Some(err) = value.get("error") {
                                    if err
                                        .get("code")
                                        .and_then(|c| c.as_i64())
                                        == Some(30003)
                                    {
                                        if stream.connection().suppressing_already_subscribed() {
                                            continue;
                                        }
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
                                        log_action("⚠️  Invalid nonce detected; refreshing signer state");
                                        let now = Instant::now();
                                        for api in [last_cancel_api_key, last_bid_api_key, last_ask_api_key]
                                            .into_iter()
                                            .flatten()
                                        {
                                            if let Some(prev) = nonce_refresh_times.get(&api) {
                                                if now.duration_since(*prev) <= NONCE_REPEAT_THRESHOLD {
                                                    if let Err(err) = signer.acknowledge_nonce_failure(api).await {
                                                        log_action(&format!("⚠️  Failed to acknowledge nonce failure for api_key {}: {err}", api));
                                                    } else {
                                                        log_action(&format!("⚠️  Acknowledged repeated nonce failure for api_key {}", api));
                                                    }
                                                }
                                            }
                                            match signer.refresh_nonce(api).await {
                                                Ok(()) => {
                                                    nonce_refresh_times.insert(api, now);
                                                }
                                                Err(refresh_err) => {
                                                    log_action(&format!("⚠️  Failed to refresh nonce for api_key {}: {refresh_err}", api));
                                                }
                                            }
                                        }
                                        last_cancel_api_key = None;
                                        last_bid_api_key = None;
                                        last_ask_api_key = None;
                                        market_watchdog.mark_recovered();
                                        continue;
                                    }
                                } else if let Some(code) = value.get("code") {
                                    if code.as_i64() == Some(30003) {
                                        if stream.connection().suppressing_already_subscribed() {
                                            continue;
                                        }
                                    }
                                    log_action(&format!("WS status: {} raw={}", code, truncate(&raw, 200)));
                                    if code.as_i64() == Some(21104) {
                                        log_action("⚠️  Invalid nonce status detected; refreshing signer state");
                                        let now = Instant::now();
                                        for api in [last_cancel_api_key, last_bid_api_key, last_ask_api_key]
                                            .into_iter()
                                            .flatten()
                                        {
                                            if let Some(prev) = nonce_refresh_times.get(&api) {
                                                if now.duration_since(*prev) <= NONCE_REPEAT_THRESHOLD {
                                                    if let Err(err) = signer.acknowledge_nonce_failure(api).await {
                                                        log_action(&format!("⚠️  Failed to acknowledge nonce failure for api_key {}: {err}", api));
                                                    } else {
                                                        log_action(&format!("⚠️  Acknowledged repeated nonce failure for api_key {}", api));
                                                    }
                                                }
                                            }
                                            match signer.refresh_nonce(api).await {
                                                Ok(()) => {
                                                    nonce_refresh_times.insert(api, now);
                                                }
                                                Err(refresh_err) => {
                                                    log_action(&format!("⚠️  Failed to refresh nonce for api_key {}: {refresh_err}", api));
                                                }
                                            }
                                        }
                                        last_cancel_api_key = None;
                                        last_bid_api_key = None;
                                        last_ask_api_key = None;
                                        market_watchdog.mark_recovered();
                                        continue;
                                    }
                                }
                            }
                        }
                        Ok(WsEvent::Connected) => {
                            last_any_message = Instant::now();
                            if let Some(start) = reconnect_start {
                                log_action(&format!("  └─ Socket handshake completed in {:?}", start.elapsed()));
                                reconnect_connected = Some(Instant::now());
                            } else {
                                log_action("WebSocket (tx) connected");
                            }
                        }
                        Ok(_other) => {
                            last_any_message = Instant::now();
                        }
                        Err(err) => {
                            log_action(&format!("❌ WebSocket error: {err}"));
                            reconnects += 1;
                            break;
                        }
                    }
                }
            }
        }
    }
}

async fn connect_stream(
    client: &LighterClient,
    auth_token: &str,
    account_index: i64,
) -> Result<lighter_client::ws_client::WsStream> {
    let mut ws = client
        .ws()
        .subscribe_order_book(MarketId::new(MARKET_ID))
        .subscribe_market_stats(MarketId::new(MARKET_ID))
        .subscribe_transactions()
        .subscribe_account_all_orders(AccountId::new(account_index))
        .connect()
        .await
        .context("Failed to establish WebSocket stream")?;
    ws.connection_mut().set_auth_token(auth_token.to_string());
    tokio::time::sleep(Duration::from_millis(200)).await;
    log_action("WebSocket connected & authenticated");
    Ok(ws)
}

fn parse_price(text: &str) -> Result<f64> {
    text.parse::<f64>()
        .map_err(|err| anyhow!("Invalid price '{text}': {err}"))
}

fn safe_parse(text: &str) -> Option<f64> {
    text.parse::<f64>().ok()
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

fn derive_bbo_guarded(
    now: Instant,
    book: &lighter_client::ws_client::OrderBookState,
    guard: &mut TobGuard,
) -> Option<(f64, f64, bool)> {
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

    let mut best_bid = bids.first()?.0;
    let mut best_ask = asks.first()?.0;

    guard.on_frame_start(now, best_bid, best_ask);

    let mut used_skip = false;

    if guard.ask_is_suspect(now, best_bid, best_ask) {
        if let Some(clean) = asks
            .iter()
            .skip(1)
            .take(MAX_LEVELS_TO_SCAN)
            .filter(|(_, qty)| *qty > 0.0)
            .map(|(price, _)| *price)
            .find(|price| *price > best_bid + PRICE_EPS)
        {
            best_ask = clean;
            used_skip = true;
        }
    }

    if best_bid >= best_ask && guard.bid_is_suspect(now, best_bid, best_ask) {
        if let Some(clean) = bids
            .iter()
            .skip(1)
            .take(MAX_LEVELS_TO_SCAN)
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

    if guard.should_escalate() {
        return None;
    }

    Some((best_bid, best_ask, used_skip))
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
    println!("  SIMPLE TWO-SIDED MM (v2 — no guardrails)");
    println!("══════════════════════════════════════════════════════════════\n");
}

fn log_action(message: &str) {
    println!(
        "[{}] {}",
        Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        message
    );
}
