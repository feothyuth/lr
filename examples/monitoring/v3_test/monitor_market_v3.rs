//! Order-book monitor that shows raw vs guarded BBO snapshots similar to the sample screenshot.

use anyhow::Result;
use chrono::Local;
use futures_util::StreamExt;
use lighter_client::ws_client::{OrderBookLevel, OrderBookState, WsEvent};
use std::time::{Duration, Instant};
#[path = "../../common/example_context.rs"]
mod common;
use common::ExampleContext;

const GUARD_MIN_TTL: Duration = Duration::from_millis(200);
const GUARD_MAX_TTL: Duration = Duration::from_millis(1500);
const GUARD_DEFAULT_TTL: Duration = Duration::from_millis(200);
const GUARD_TTL_MULTIPLIER: f64 = 4.0;
const GUARD_EMA_ALPHA: f64 = 0.2;
const GUARD_MAX_SCAN_LEVELS: usize = 8;
const GUARD_MAX_CONSEC_CROSSED: u32 = 6;
const GUARD_MIN_UPDATES: u32 = 3;
const PRICE_EPS: f64 = 1e-9;

#[derive(Clone, Copy)]
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

    fn ask_is_suspect(
        &self,
        now: Instant,
        best_bid: f64,
        best_ask: f64,
        cfg: &TobGuardConfig,
    ) -> bool {
        best_ask <= best_bid + PRICE_EPS
            && self
                .last_ask_changed_at
                .map(|ts| now.duration_since(ts) >= self.ttl_from_ema(self.ema_ask_interval, cfg))
                .unwrap_or(false)
            && self.updates_since_ask_change >= cfg.min_updates
    }

    fn bid_is_suspect(
        &self,
        now: Instant,
        best_bid: f64,
        best_ask: f64,
        cfg: &TobGuardConfig,
    ) -> bool {
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

enum GuardOutcome {
    Bbo {
        bid: f64,
        ask: f64,
        skipped: bool,
        stale_bid: bool,
        stale_ask: bool,
    },
    Escalate {
        reason: &'static str,
    },
}

fn level_quantity(level: &OrderBookLevel) -> f64 {
    if let Some(remaining) = level
        .remaining_base_amount
        .as_deref()
        .and_then(|text| text.parse::<f64>().ok())
    {
        remaining
    } else {
        level.size.parse::<f64>().unwrap_or(0.0)
    }
}

fn level_active(level: &OrderBookLevel) -> bool {
    level_quantity(level) > 0.0
}

fn derive_bbo_guarded(
    now: Instant,
    book: &OrderBookState,
    cfg: &TobGuardConfig,
    guard: &mut TobGuard,
) -> GuardOutcome {
    let bids: Vec<(f64, f64)> = book
        .bids
        .iter()
        .filter(|lvl| level_active(lvl))
        .filter_map(|lvl| {
            lvl.price
                .parse::<f64>()
                .ok()
                .map(|p| (p, level_quantity(lvl)))
        })
        .collect();

    let asks: Vec<(f64, f64)> = book
        .asks
        .iter()
        .filter(|lvl| level_active(lvl))
        .filter_map(|lvl| {
            lvl.price
                .parse::<f64>()
                .ok()
                .map(|p| (p, level_quantity(lvl)))
        })
        .collect();

    let Some(mut best_bid) = bids.first().map(|(p, _)| *p) else {
        return GuardOutcome::Escalate { reason: "no bids" };
    };
    let Some(mut best_ask) = asks.first().map(|(p, _)| *p) else {
        return GuardOutcome::Escalate { reason: "no asks" };
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
            reason: "crossed_after_guard",
        };
    }

    GuardOutcome::Bbo {
        bid: best_bid,
        ask: best_ask,
        skipped: used_skip,
        stale_bid: bid_suspect,
        stale_ask: ask_suspect,
    }
}

fn extract_raw_bbo(book: &OrderBookState) -> Option<(f64, f64)> {
    let mut best_bid: Option<f64> = None;
    for price in book
        .bids
        .iter()
        .filter(|lvl| level_active(lvl))
        .filter_map(|lvl| lvl.price.parse::<f64>().ok())
    {
        best_bid = Some(best_bid.map_or(price, |p| p.max(price)));
    }

    let mut best_ask: Option<f64> = None;
    for price in book
        .asks
        .iter()
        .filter(|lvl| level_active(lvl))
        .filter_map(|lvl| lvl.price.parse::<f64>().ok())
    {
        best_ask = Some(best_ask.map_or(price, |p| p.min(price)));
    }

    match (best_bid, best_ask) {
        (Some(bid), Some(ask)) => Some((bid, ask)),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt().with_env_filter("info").init();

    println!("{}", "═".repeat(80));
    println!("📊 Order Book Monitor (raw vs guarded) — real-time");
    println!("{}", "═".repeat(80));

    let ctx = ExampleContext::initialise(Some("monitor_market_v3")).await?;
    let market = ctx.market_id();

    let mut stream = ctx
        .ws_builder()
        .subscribe_order_book(market)
        .connect()
        .await?;
    let auth_token = ctx.auth_token().await?;
    stream.connection_mut().set_auth_token(auth_token);

    let cfg = TobGuardConfig::default();
    let mut guard = TobGuard::new();

    while let Some(event) = stream.next().await {
        match event {
            Ok(WsEvent::OrderBook(ob)) => {
                let now = Instant::now();
                let raw_bbo = match extract_raw_bbo(&ob.state) {
                    Some(pair) => pair,
                    None => continue,
                };

                match derive_bbo_guarded(now, &ob.state, &cfg, &mut guard) {
                    GuardOutcome::Bbo {
                        bid,
                        ask,
                        skipped,
                        stale_bid,
                        stale_ask,
                    } => {
                        let ts = Local::now().format("%H:%M:%S%.3f");
                        let (raw_bid, raw_ask) = raw_bbo;
                        let raw_spread = raw_ask - raw_bid;
                        let raw_crossed = raw_ask <= raw_bid + PRICE_EPS;
                        let guard_spread = ask - bid;
                        println!(
                            "[{ts}] RAW   : bid={:.6} ask={:.6} spread={:.6}{}",
                            raw_bid,
                            raw_ask,
                            raw_spread,
                            if raw_crossed { "  ⚠️" } else { "" }
                        );
                        println!(
                            "        GUARD : bid={:.6} ask={:.6} spread={:.6} skipped={} stale_bid={} stale_ask={} crossed_frames={}",
                            bid,
                            ask,
                            guard_spread,
                            skipped,
                            stale_bid,
                            stale_ask,
                            guard.crossed_frames
                        );
                        println!();
                    }
                    GuardOutcome::Escalate { reason } => {
                        let ts = Local::now().format("%H:%M:%S%.3f");
                        let (raw_bid, raw_ask) = raw_bbo;
                        let raw_spread = raw_ask - raw_bid;
                        println!(
                            "[{ts}] RAW   : bid={:.6} ask={:.6} spread={:.6}  ⚠️",
                            raw_bid, raw_ask, raw_spread
                        );
                        println!("        GUARD : escalation={reason}");
                        println!();
                        guard.mark_fresh(now, raw_bid, raw_ask);
                    }
                }
            }
            Ok(WsEvent::Connected) => println!("✅ connected"),
            Ok(WsEvent::Pong) => (),
            Ok(_) => (),
            Err(err) => {
                println!("❌ WebSocket error: {err}");
                break;
            }
        }
    }

    Ok(())
}
