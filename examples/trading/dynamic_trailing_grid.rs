//! Dynamic trailing grid bot with full-reset policy.
//!
//! Key behaviours (summarised from `grid_details.md`):
//! - **Volatility-aware spacing**: ATR-derived spacing that widens during fast moves.
//! - **Full reset on every fill**: as soon as any order fills, cancel everything and rebuild.
//! - **Trailing anchor**: default is to re-center the grid on the **filled price** to
//!   maximise follow-through volume; alternative mid sources can be selected via env vars.
//! - **Batch order submission**: bids/asks are signed locally and pushed in one WebSocket
//!   batch for minimum latency.
//!
//! Environment overrides (all optional):
//! - `GRID_LEVELS_PER_SIDE` (usize, default 5)
//! - `GRID_ORDER_SIZE` (base units, default 0.0002)
//! - `GRID_ATR_WINDOW` (usize, default 32)
//! - `GRID_ATR_MULTIPLIER` (f64, default 1.5)
//! - `GRID_MIN_SPACING_BPS` (f64, default 2.0 → 0.02%)
//! - `GRID_TRAIL_PCT` (f64, default 0.05 → ±5% band)
//! - `GRID_MID_SOURCE` (`filled`, `market`, `trade`)
//! - Standard `LIGHTER_*` vars for auth/market selection.

#[path = "../common/example_context.rs"]
mod common;

use anyhow::{anyhow, Context, Result};
use common::{connect_private_stream, sign_cancel_all_for_ws, ExampleContext};
use futures_util::StreamExt;
use lighter_client::{
    models::OrderBookOrders,
    signer_client::{SignedPayload, SignerClient},
    trading_helpers::{calculate_grid_levels, scale_price_to_int, scale_size_to_int},
    transactions::CreateOrder,
    tx_executor::send_batch_tx_ws,
    types::MarketId,
    ws_client::{AccountEventEnvelope, OrderBookEvent, TradeSide, WsConnection, WsEvent},
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    convert::TryFrom,
    env,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::{interval, sleep, MissedTickBehavior};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    println!("\n{}", "═".repeat(88));
    println!("🤖 Dynamic Trailing Grid Bot (full reset, ATR spacing)");
    println!("{}", "═".repeat(88));

    let ctx = ExampleContext::initialise(Some("dynamic_trailing_grid")).await?;
    let market = ctx.market_id();
    let account = ctx.account_id();
    let config = GridBotConfig::from_env(market);
    let mut bot = DynamicGridBot::bootstrap(&ctx, config).await?;

    println!(
        "📊 Market: {} (ID {})",
        bot.metadata.symbol,
        bot.config.market_id.into_inner()
    );
    println!(
        "   Levels/side: {} | Order size: {} | Mid source: {:?}",
        bot.config.levels_per_side, bot.config.order_size, bot.config.mid_source
    );
    println!(
        "   ATR window: {} | ATR mult: {:.2} | Min spacing: {:.4}%",
        bot.config.atr_window,
        bot.config.atr_multiplier,
        bot.config.min_spacing_pct * 100.0
    );
    println!();

    // Create separate WebSocket connections (like mm_hawkes.rs)
    println!("📡 Creating separate WebSocket connections...");

    // 1. Order book stream
    let mut order_book_stream = ctx
        .ws_builder()
        .subscribe_order_book(market)
        .connect()
        .await?;
    println!("✅ Order book stream connected");

    // 2. Market stats stream
    let mut market_stats_stream = ctx
        .ws_builder()
        .subscribe_market_stats(market)
        .connect()
        .await?;
    println!("✅ Market stats stream connected");

    // 3. Trade stream (public market trades)
    let mut trade_stream = ctx
        .ws_builder()
        .subscribe_trade(market)
        .connect()
        .await?;
    println!("✅ Trade stream connected");

    // 4. Account stream (orders + trades) - authenticated
    let (mut account_stream, _) = connect_private_stream(
        &ctx,
        ctx.ws_builder()
            .subscribe_account_market_trades(market, account)
            .subscribe_account_all_orders(account)
    ).await?;
    println!("✅ Account stream connected (authenticated)");

    // 5. Transaction stream - authenticated, separate connection for sending/monitoring txs
    let tx_builder = ctx.ws_builder().subscribe_account_tx(account);
    let (mut tx_stream, _) = connect_private_stream(&ctx, tx_builder).await?;
    println!("✅ Transaction stream connected (authenticated)");

    // Wait for all streams to complete handshake
    println!("⏳ Waiting for all WebSocket handshakes to complete...");
    let mut handshakes_complete = 0;

    // Wait for each stream's Connected event
    while let Some(event) = order_book_stream.next().await {
        if matches!(event?, WsEvent::Connected) {
            handshakes_complete += 1;
            break;
        }
    }

    while let Some(event) = market_stats_stream.next().await {
        if matches!(event?, WsEvent::Connected) {
            handshakes_complete += 1;
            break;
        }
    }

    while let Some(event) = trade_stream.next().await {
        if matches!(event?, WsEvent::Connected) {
            handshakes_complete += 1;
            break;
        }
    }

    while let Some(event) = account_stream.next().await {
        if matches!(event?, WsEvent::Connected) {
            handshakes_complete += 1;
            break;
        }
    }

    while let Some(event) = tx_stream.next().await {
        if matches!(event?, WsEvent::Connected) {
            handshakes_complete += 1;
            break;
        }
    }

    if handshakes_complete != 5 {
        return Err(anyhow!("Not all WebSocket connections established"));
    }

    println!("🔗 All 5 WebSocket connections established!");

    // NOW it's safe to deploy the grid using the tx connection
    bot.deploy_initial_grid(tx_stream.connection_mut()).await?;
    println!("🚀 Grid deployed — waiting for fills to trigger resets\n");

    let mut heartbeat = interval(Duration::from_secs(30));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    println!("🔁 DEBUG: Entering main event loop");
    loop {
        tokio::select! {
            // Order book events
            order_event = order_book_stream.next() => {
                match order_event {
                    Some(Ok(event)) => {
                        println!("📊 Order book event: {}", event_label(&event));
                        if let Err(err) = bot.handle_event(event, tx_stream.connection_mut()).await {
                            eprintln!("⚠️  Order book handler error: {err:#}");
                        }
                    }
                    Some(Err(err)) => {
                        eprintln!("❌ Order book stream error: {err}");
                        return Err(err.into());
                    }
                    None => {
                        println!("🔌 Order book stream closed");
                        break;
                    }
                }
            }

            // Market stats events
            stats_event = market_stats_stream.next() => {
                match stats_event {
                    Some(Ok(event)) => {
                        println!("📈 Market stats event: {}", event_label(&event));
                        if let Err(err) = bot.handle_event(event, tx_stream.connection_mut()).await {
                            eprintln!("⚠️  Market stats handler error: {err:#}");
                        }
                    }
                    Some(Err(err)) => {
                        eprintln!("❌ Market stats stream error: {err}");
                        return Err(err.into());
                    }
                    None => {
                        println!("🔌 Market stats stream closed");
                        break;
                    }
                }
            }

            // Trade events (public market trades)
            trade_event = trade_stream.next() => {
                match trade_event {
                    Some(Ok(event)) => {
                        println!("💱 Trade event: {}", event_label(&event));
                        if let Err(err) = bot.handle_event(event, tx_stream.connection_mut()).await {
                            eprintln!("⚠️  Trade handler error: {err:#}");
                        }
                    }
                    Some(Err(err)) => {
                        eprintln!("❌ Trade stream error: {err}");
                        return Err(err.into());
                    }
                    None => {
                        println!("🔌 Trade stream closed");
                        break;
                    }
                }
            }

            // Account events (fills and orders)
            account_event = account_stream.next() => {
                match account_event {
                    Some(Ok(event)) => {
                        println!("👤 Account event: {}", event_label(&event));
                        if let Err(err) = bot.handle_event(event, tx_stream.connection_mut()).await {
                            eprintln!("⚠️  Account handler error: {err:#}");
                        }
                    }
                    Some(Err(err)) => {
                        eprintln!("❌ Account stream error: {err}");
                        return Err(err.into());
                    }
                    None => {
                        println!("🔌 Account stream closed");
                        break;
                    }
                }
            }

            // Transaction events (order confirmations)
            tx_event = tx_stream.next() => {
                match tx_event {
                    Some(Ok(event)) => {
                        println!("📤 Transaction event: {}", event_label(&event));
                        // Transaction events are typically just confirmations, log them
                        match event {
                            WsEvent::Connected => println!("📤 Tx stream reconnected"),
                            WsEvent::Pong => {},
                            other => println!("📤 Tx event: {:?}", other),
                        }
                    }
                    Some(Err(err)) => {
                        eprintln!("❌ Transaction stream error: {err}");
                        return Err(err.into());
                    }
                    None => {
                        println!("🔌 Transaction stream closed");
                        break;
                    }
                }
            }
            _ = heartbeat.tick() => {
                let current_mid = bot.get_mid_price();
                let source_detail = match bot.config.mid_source {
                    MidPriceSource::FilledPrice => {
                        if bot.last_fill_price.is_some() {
                            format!("My Fill ${:.2}", current_mid)
                        } else {
                            format!("Market Trade ${:.2} (no fills yet)", current_mid)
                        }
                    }
                    MidPriceSource::MarketMid => format!("BBO Mid ${:.2}", current_mid),
                    MidPriceSource::LastTrade => format!("Market Trade ${:.2}", current_mid),
                };
                println!(
                    "⏳ Heartbeat: {} | Book: ${:.2}/${:.2} | ATR {:.5}",
                    source_detail,
                    bot.latest_order_book.as_ref().map(|b| b.best_bid).unwrap_or(0.0),
                    bot.latest_order_book.as_ref().map(|b| b.best_ask).unwrap_or(0.0),
                    bot.atr.value().unwrap_or(0.0)
                );
            }
        }
    }

    Ok(())
}

// === Configuration & helpers =================================================

#[derive(Debug, Clone, Copy)]
enum MidPriceSource {
    FilledPrice,
    MarketMid,
    LastTrade,
}

impl std::fmt::Debug for GridBotConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GridBotConfig")
            .field("levels_per_side", &self.levels_per_side)
            .field("order_size", &self.order_size)
            .field("atr_window", &self.atr_window)
            .field("atr_multiplier", &self.atr_multiplier)
            .field("min_spacing_pct", &self.min_spacing_pct)
            .field("trail_pct", &self.trail_pct)
            .field("mid_source", &self.mid_source)
            .finish()
    }
}

#[derive(Clone)]
struct GridBotConfig {
    market_id: MarketId,
    levels_per_side: usize,
    order_size: f64,
    atr_window: usize,
    atr_multiplier: f64,
    min_spacing_pct: f64,
    trail_pct: f64,
    mid_source: MidPriceSource,
}

impl GridBotConfig {
    fn from_env(default_market: MarketId) -> Self {
        Self {
            market_id: default_market,
            levels_per_side: env_usize("GRID_LEVELS_PER_SIDE", 5).max(1),
            order_size: env_f64("GRID_ORDER_SIZE", 0.0002),
            atr_window: env_usize("GRID_ATR_WINDOW", 32).max(4),
            atr_multiplier: env_f64("GRID_ATR_MULTIPLIER", 1.5).max(0.5),
            min_spacing_pct: env_f64("GRID_MIN_SPACING_BPS", 1.0) / 10_000.0,
            trail_pct: env_f64("GRID_TRAIL_PCT", 0.05).max(0.01),
            mid_source: parse_mid_source(),
        }
    }
}

fn parse_mid_source() -> MidPriceSource {
    match env::var("GRID_MID_SOURCE")
        .unwrap_or_else(|_| "filled".to_string())
        .to_lowercase()
        .as_str()
    {
        "market" | "market_mid" => MidPriceSource::MarketMid,
        "trade" | "last_trade" => MidPriceSource::LastTrade,
        _ => MidPriceSource::FilledPrice,
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default)
}

// === Bot implementation ======================================================

struct DynamicGridBot<'a> {
    ctx: &'a ExampleContext,
    signer: &'a SignerClient,
    config: GridBotConfig,
    metadata: MarketMetadata,
    atr: AtrTracker,
    trailing: TrailingRange,
    last_book_mid: f64,    // Mid from order book BBO
    last_mark_price: f64,  // Mark price from market stats
    last_trade: f64,       // Last market trade price
    last_fill_price: Option<f64>,  // Last actual fill of our orders
    last_processed_fill: Option<(f64, f64)>,  // (price, size) to detect duplicates
    base_qty_units: i64,
    next_client_id: i64,
    order_tracker: OrderTracker,
    latest_order_book: Option<OrderBookSnapshot>,
    resetting_grid: bool,  // Flag to prevent duplicate fill processing
}

struct MarketMetadata {
    symbol: String,
    price_decimals: u8,
    size_decimals: u8,
}

#[derive(Clone, Copy)]
struct OrderBookSnapshot {
    best_bid: f64,
    best_ask: f64,
}

impl<'a> DynamicGridBot<'a> {
    async fn bootstrap(ctx: &'a ExampleContext, config: GridBotConfig) -> Result<Self> {
        let signer = ctx.signer()?;
        let client = ctx.client();
        let book = client.orders().book(config.market_id, 50).await?;

        let (best_bid, best_ask) =
            extract_best_prices(&book).context("order book missing bids/asks for initial mid")?;
        let initial_mid = (best_bid + best_ask) / 2.0;

        let metadata = client
            .orders()
            .book_details(Some(config.market_id))
            .await?
            .order_book_details
            .into_iter()
            .next()
            .map(|detail| MarketMetadata {
                symbol: detail.symbol,
                price_decimals: detail.price_decimals as u8,
                size_decimals: detail.size_decimals as u8,
            })
            .context("missing market metadata")?;

        let base_units = scale_size_to_int(config.order_size, metadata.size_decimals).max(1);
        let atr = AtrTracker::new(config.atr_window);
        let trailing = TrailingRange::new(initial_mid, config.trail_pct);
        let next_client_id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;

        Ok(Self {
            ctx,
            signer,
            config,
            metadata,
            atr,
            trailing,
            last_book_mid: initial_mid,
            last_mark_price: initial_mid,
            last_trade: initial_mid,
            last_fill_price: None,
            base_qty_units: base_units,
            next_client_id,
            order_tracker: OrderTracker::default(),
            latest_order_book: None,
            last_processed_fill: None,
            resetting_grid: false,
        })
    }

    async fn handle_event(&mut self, event: WsEvent, connection: &mut WsConnection) -> Result<()> {
        // DEBUG: Log ALL events
        println!("🔔 Event: {}", event_label(&event));

        match event {
            WsEvent::OrderBook(book_event) => {
                if let Some((bid, ask)) = best_from_event(&book_event) {
                    let book_mid = (bid + ask) / 2.0;
                    self.last_book_mid = book_mid;
                    self.atr.update(book_mid);
                    self.latest_order_book = Some(OrderBookSnapshot {
                        best_bid: bid,
                        best_ask: ask,
                    });
                }
            }
            WsEvent::MarketStats(stats) => {
                if let Ok(mark) = stats.market_stats.mark_price.parse::<f64>() {
                    self.last_mark_price = mark;
                }
            }
            WsEvent::Trade(trade_event) => {
                for trade in trade_event.trades {
                    if let Ok(price) = trade.price.parse::<f64>() {
                        self.last_trade = price;
                        self.atr.update(price);
                    }
                }
            }
            WsEvent::Account(envelope) => {
                println!("📨 Received Account event for account: {}", envelope.account.into_inner());

                if envelope.account != self.ctx.account_id() {
                    println!("⚠️  Different account, skipping");
                    return Ok(());
                }

                // Skip if we're already resetting the grid
                if self.resetting_grid {
                    println!("⏸️  Already resetting grid, skipping");
                    return Ok(());
                }

                // Parse trades from SDK-filtered event
                let raw_json = envelope.event.into_inner();

                println!("📄 Raw event JSON:");
                println!("{}\n", serde_json::to_string_pretty(&raw_json)
                    .unwrap_or_else(|_| "parse error".to_string()));

                // Extract trades - SDK returns {"trades": {"{MARKET_INDEX}": [Trade]}}
                if let Some(trades_value) = raw_json.get("trades") {
                    // Trades is an object keyed by market_id
                    if let Some(trades_obj) = trades_value.as_object() {
                        for (market_key, market_trades_value) in trades_obj {
                            println!("📊 Market {}: checking trades", market_key);

                            if let Some(arr) = market_trades_value.as_array() {
                                println!("  ✅ Found {} trades", arr.len());

                                for trade in arr {
                                    // Get trade details
                                    if let (Some(price_str), Some(size_str), Some(bid_account), Some(ask_account)) =
                                        (trade.get("price").and_then(|v| v.as_str()),
                                         trade.get("size").and_then(|v| v.as_str()),
                                         trade.get("bid_account_id").and_then(|v| v.as_i64()),
                                         trade.get("ask_account_id").and_then(|v| v.as_i64())) {

                                        if let (Ok(price), Ok(size)) =
                                            (price_str.parse::<f64>(), size_str.parse::<f64>()) {

                                            // Determine our side
                                            let account_id = self.ctx.account_id().into_inner() as i64;
                                            let side = if bid_account == account_id {
                                                "BUY"
                                            } else if ask_account == account_id {
                                                "SELL"
                                            } else {
                                                continue; // Not our trade
                                            };

                                            // Check if this is a duplicate fill
                                            if let Some((last_price, last_size)) = self.last_processed_fill {
                                                if (price - last_price).abs() < 0.01 && (size - last_size).abs() < 0.000001 {
                                                    continue;
                                                }
                                            }

                                            println!("💥 Fill detected: {} {:.6} @ ${:.2}", side, size, price);

                                            self.last_fill_price = Some(price);
                                            self.last_processed_fill = Some((price, size));
                                            self.resetting_grid = true;

                                            let fill = FillEvent {
                                                price,
                                                size,
                                                side: side.to_string(),
                                            };
                                            let result = self.reset_after_fill(&fill, connection).await;
                                            self.resetting_grid = false;
                                            result?;
                                            return Ok(());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            WsEvent::Closed(frame) => {
                println!("🔌 WebSocket closed: {:?}", frame);
            }
            WsEvent::Connected => println!("🔗 Stream connected"),
            WsEvent::Pong => {}
            other => {
                println!("ℹ️  Received other event: {}", event_label(&other));
            }
        }
        Ok(())
    }

    fn get_mid_price(&self) -> f64 {
        match self.config.mid_source {
            MidPriceSource::FilledPrice => {
                // Use actual fill price if available, otherwise fall back to last trade
                self.last_fill_price.unwrap_or(self.last_trade)
            }
            MidPriceSource::MarketMid => self.last_book_mid,
            MidPriceSource::LastTrade => self.last_trade,
        }
    }

    async fn deploy_initial_grid(&mut self, connection: &mut WsConnection) -> Result<()> {
        let initial_anchor = self.get_mid_price();
        self.place_grid(initial_anchor, connection, 0).await
    }

    async fn reset_after_fill(
        &mut self,
        fill: &FillEvent,
        connection: &mut WsConnection,
    ) -> Result<()> {
        // Cancel existing orders first
        self.cancel_all(connection).await?;

        // Wait a bit for cancel to propagate and market to settle
        sleep(Duration::from_millis(200)).await;

        // Determine anchor price
        let anchor = match self.config.mid_source {
            MidPriceSource::FilledPrice => fill.price,
            MidPriceSource::MarketMid => self.last_book_mid,
            MidPriceSource::LastTrade => self.last_trade,
        };

        self.trailing.update(anchor);
        let anchor = self
            .sanity_guard(anchor, &self.latest_order_book)
            .unwrap_or(anchor);

        self.place_grid(anchor, connection, 0).await
    }

    async fn cancel_all(&mut self, connection: &mut WsConnection) -> Result<()> {
        let mut attempt = 0;
        loop {
            let payload = sign_cancel_all_for_ws(self.ctx, 0, 0).await?;
            let results = send_batch_tx_ws(connection, vec![payload]).await?;
            if results.first().copied() == Some(true) {
                println!("🧹 Existing orders cancelled");
                break;
            }

            attempt += 1;
            println!("⚠️  Cancel-all rejected (attempt {attempt}).");
            if attempt >= 2 {
                println!("⚠️  Giving up on cancel-all after {attempt} attempts.");
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }

        self.order_tracker.clear();
        Ok(())
    }

    async fn place_grid(
        &mut self,
        anchor: f64,
        connection: &mut WsConnection,
        attempt: u8,
    ) -> Result<()> {
        let mut current_anchor = anchor;
        let mut attempt_count = attempt;

        loop {
            let spacing_pct = self.spacing_pct(current_anchor);
            let (bid_prices, ask_prices) =
                calculate_grid_levels(current_anchor, self.config.levels_per_side, spacing_pct);

            // Show current book state
            let book_info = if let Some(snap) = &self.latest_order_book {
                format!(" | Book: ${:.2}/${:.2}", snap.best_bid, snap.best_ask)
            } else {
                String::new()
            };

            println!(
                "🧱 Rebuilding grid @ ${:.2} (spacing {:.4}%){}",
                current_anchor,
                spacing_pct * 100.0,
                book_info
            );
            print_side("BID", &bid_prices, self.config.order_size);
            print_side("ASK", &ask_prices, self.config.order_size);

            // Pre-flight check: verify orders won't cross
            if let Some(snap) = &self.latest_order_book {
                if let Some(closest_bid) = bid_prices.first() {
                    if *closest_bid >= snap.best_ask {
                        println!(
                            "⚠️  Pre-flight check failed: bid ${:.2} would cross ask ${:.2}",
                            closest_bid, snap.best_ask
                        );
                        if attempt_count >= 3 {
                            return Err(anyhow!(
                                "Cannot place grid - spread too tight after {} attempts",
                                attempt_count + 1
                            ));
                        }
                        // Widen the anchor away from the ask
                        let mid = (snap.best_bid + snap.best_ask) / 2.0;
                        current_anchor = mid;
                        attempt_count += 1;
                        sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                }
                if let Some(closest_ask) = ask_prices.first() {
                    if *closest_ask <= snap.best_bid {
                        println!(
                            "⚠️  Pre-flight check failed: ask ${:.2} would cross bid ${:.2}",
                            closest_ask, snap.best_bid
                        );
                        if attempt_count >= 3 {
                            return Err(anyhow!(
                                "Cannot place grid - spread too tight after {} attempts",
                                attempt_count + 1
                            ));
                        }
                        // Widen the anchor away from the bid
                        let mid = (snap.best_bid + snap.best_ask) / 2.0;
                        current_anchor = mid;
                        attempt_count += 1;
                        sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                }
            }

            let mut signed_orders = Vec::with_capacity(bid_prices.len() + ask_prices.len());
            signed_orders.extend(self.sign_side_orders(&bid_prices, false).await?);
            signed_orders.extend(self.sign_side_orders(&ask_prices, true).await?);

            if signed_orders.is_empty() {
                return Ok(());
            }

            let payloads: Vec<(u8, String)> = signed_orders
                .iter()
                .map(|signed| (signed.tx_type() as u8, signed.payload().to_string()))
                .collect();

            let results = send_batch_tx_ws(connection, payloads).await?;
            let accepted = results.iter().filter(|&&ok| ok).count();
            println!(
                "📤 Grid batch submitted: {}/{} accepted",
                accepted,
                results.len()
            );

            // If all orders accepted, we're done
            if accepted == results.len() {
                return Ok(());
            }

            // If some orders rejected and we haven't retried yet, try with live mid
            if accepted == 0 && attempt_count < 3 {
                if let Some(fallback) = self.fallback_mid_from_snapshot() {
                    println!(
                        "⚠️  Batch rejected (post-only). Retrying at live mid ${:.2}",
                        fallback
                    );
                    current_anchor = fallback;
                    attempt_count += 1;
                    sleep(Duration::from_millis(100)).await; // Wait for market to update
                    continue;
                }
            }

            // Partial acceptance or max retries - return with warning
            if accepted > 0 {
                println!("⚠️  Only {}/{} orders placed, continuing anyway", accepted, results.len());
                return Ok(());
            }

            // Total failure after retries
            return Err(anyhow!("Failed to place any orders after {} attempts", attempt_count + 1));
        }
    }

    fn sanity_guard(&self, anchor: f64, snapshot: &Option<OrderBookSnapshot>) -> Option<f64> {
        let snap = snapshot.as_ref()?;
        let spacing_pct = self.spacing_pct(anchor);
        let (bid_prices, ask_prices) =
            calculate_grid_levels(anchor, self.config.levels_per_side, spacing_pct);

        if let Some(lowest_ask) = ask_prices.first() {
            if *lowest_ask <= snap.best_bid {
                let mid = (snap.best_bid + snap.best_ask) / 2.0;
                println!(
                    "⚠️  Sanity guard: ask ${:.2} would cross bid ${:.2}. Using mid ${:.2}",
                    lowest_ask, snap.best_bid, mid
                );
                return Some(mid);
            }
        }

        if let Some(highest_bid) = bid_prices.first() {
            if *highest_bid >= snap.best_ask {
                let mid = (snap.best_bid + snap.best_ask) / 2.0;
                println!(
                    "⚠️  Sanity guard: bid ${:.2} would cross ask ${:.2}. Using mid ${:.2}",
                    highest_bid, snap.best_ask, mid
                );
                return Some(mid);
            }
        }

        None
    }

    fn fallback_mid_from_snapshot(&self) -> Option<f64> {
        self.latest_order_book
            .as_ref()
            .map(|snap| (snap.best_bid + snap.best_ask) / 2.0)
    }

    async fn sign_side_orders(
        &mut self,
        prices: &[f64],
        is_ask: bool,
    ) -> Result<Vec<SignedPayload<CreateOrder>>> {
        let mut signed = Vec::with_capacity(prices.len());
        for price in prices {
            let ticks = scale_price_to_int(*price, self.metadata.price_decimals);
            let price_i32 = i32::try_from(ticks)
                .map_err(|_| anyhow!("price {} exceeds tick range for market", price))?;
            let (api_key_idx, nonce) = self.signer.next_nonce().await?;

            let signed_payload = self
                .signer
                .sign_create_order(
                    self.config.market_id.into_inner(),
                    self.next_client_id,
                    self.base_qty_units,
                    price_i32,
                    is_ask,
                    self.signer.order_type_limit(),
                    self.signer.order_time_in_force_post_only(),
                    false,
                    0,
                    -1,
                    Some(nonce),
                    Some(api_key_idx),
                )
                .await?;
            self.next_client_id += 1;
            signed.push(signed_payload);
        }
        Ok(signed)
    }

    fn spacing_pct(&self, anchor: f64) -> f64 {
        let atr_pct = self
            .atr
            .value()
            .map(|atr| (atr / anchor).clamp(0.00005, 0.01) * self.config.atr_multiplier)
            .unwrap_or(self.config.min_spacing_pct);
        atr_pct.max(self.config.min_spacing_pct)
    }
}

// === ATR + trailing helpers ==================================================

struct AtrTracker {
    window: usize,
    buffer: VecDeque<f64>,
    last_price: Option<f64>,
}

impl AtrTracker {
    fn new(window: usize) -> Self {
        Self {
            window,
            buffer: VecDeque::with_capacity(window),
            last_price: None,
        }
    }

    fn update(&mut self, price: f64) {
        if let Some(prev) = self.last_price {
            let tr = (price - prev).abs();
            if self.buffer.len() == self.window {
                self.buffer.pop_front();
            }
            self.buffer.push_back(tr);
        }
        self.last_price = Some(price);
    }

    fn value(&self) -> Option<f64> {
        if self.buffer.is_empty() {
            None
        } else {
            Some(self.buffer.iter().sum::<f64>() / self.buffer.len() as f64)
        }
    }
}

struct TrailingRange {
    lower: f64,
    upper: f64,
    trail_pct: f64,
}

impl TrailingRange {
    fn new(center: f64, trail_pct: f64) -> Self {
        let width = center * trail_pct;
        Self {
            lower: center - width,
            upper: center + width,
            trail_pct,
        }
    }

    fn update(&mut self, price: f64) {
        if price > self.upper {
            let shift = price - self.upper;
            self.upper += shift;
            self.lower += shift;
            println!("📈 Trailing range shifted up by ${:.2}", shift);
        } else if price < self.lower {
            let shift = self.lower - price;
            self.lower -= shift;
            self.upper -= shift;
            println!("📉 Trailing range shifted down by ${:.2}", shift);
        }
    }
}

// === Fill parsing ============================================================

#[derive(Debug, Clone)]
struct FillEvent {
    price: f64,
    size: f64,
    side: String,
}

#[derive(Clone)]
struct OrderState {
    remaining: f64,
    price: f64,
    side: String,
}

#[derive(Default)]
struct OrderTracker {
    orders: HashMap<i64, OrderState>,
}

impl OrderTracker {
    fn detect_order_reductions(
        &mut self,
        envelope: &AccountEventEnvelope,
        market_id: i32,
    ) -> Vec<FillEvent> {
        let mut fills = Vec::new();
        let value = envelope.event.as_value();
        let Some(orders_obj) = value.get("orders").and_then(|v| v.as_object()) else {
            self.orders.clear();
            return fills;
        };

        let market_key = market_id.to_string();
        let Some(orders) = orders_obj.get(&market_key).and_then(|v| v.as_array()) else {
            self.orders.clear();
            return fills;
        };

        let mut updated = HashMap::new();
        let mut seen = HashSet::new();

        for order_val in orders {
            let order_index = order_val
                .get("order_index")
                .and_then(|v| v.as_i64())
                .unwrap_or(-1);
            if order_index < 0 {
                continue;
            }
            seen.insert(order_index);

            let remaining = parse_numeric_field(order_val, "remaining_base_amount")
                .or_else(|| parse_numeric_field(order_val, "size"))
                .unwrap_or(0.0);
            let price = parse_numeric_field(order_val, "price").unwrap_or(0.0);
            let side = if order_val
                .get("is_ask")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                "ASK".to_string()
            } else {
                "BID".to_string()
            };

            if let Some(prev) = self.orders.get(&order_index) {
                if prev.remaining > remaining + 1e-9 {
                    fills.push(FillEvent {
                        price,
                        size: prev.remaining - remaining,
                        side: side.clone(),
                    });
                }
            }

            updated.insert(
                order_index,
                OrderState {
                    remaining,
                    price,
                    side,
                },
            );
        }

        // Orders missing from snapshot → treat as fully filled/cancelled
        for (order_index, prev_state) in self.orders.iter() {
            if !seen.contains(order_index) && prev_state.remaining > 1e-9 {
                fills.push(FillEvent {
                    price: prev_state.price,
                    size: prev_state.remaining,
                    side: prev_state.side.clone(),
                });
            }
        }

        self.orders = updated;
        fills
    }

    fn clear(&mut self) {
        self.orders.clear();
    }
}

fn parse_numeric_field(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(|v| {
        if v.is_string() {
            v.as_str()?.parse::<f64>().ok()
        } else {
            v.as_f64()
        }
    })
}

fn event_label(event: &WsEvent) -> &'static str {
    match event {
        WsEvent::Connected => "Connected",
        WsEvent::Pong => "Pong",
        WsEvent::OrderBook(_) => "OrderBook",
        WsEvent::Account(_) => "Account",
        WsEvent::MarketStats(_) => "MarketStats",
        WsEvent::Transaction(_) => "Transaction",
        WsEvent::ExecutedTransaction(_) => "ExecutedTransaction",
        WsEvent::Height(_) => "Height",
        WsEvent::Trade(_) => "Trade",
        WsEvent::BBO(_) => "BBO",
        WsEvent::Closed(_) => "Closed",
        WsEvent::Unknown(_) => "Unknown",
    }
}

// === Utility functions =======================================================

fn extract_best_prices(book: &OrderBookOrders) -> Option<(f64, f64)> {
    // Find best bid - skip orders with zero size (cancelled/filled)
    let best_bid = book.bids.iter().find_map(|lvl| {
        let size_str = &lvl.remaining_base_amount;
        // Fast path: skip common zero representations without parsing
        if size_str == "0" || size_str == "0.0" || size_str == "0.00" || size_str.is_empty() {
            return None;
        }
        // Slow path: parse to verify size > 0
        let size: f64 = size_str.parse().ok()?;
        if size > 0.0 {
            lvl.price.parse::<f64>().ok()
        } else {
            None
        }
    })?;

    // Find best ask - skip orders with zero size
    let best_ask = book.asks.iter().find_map(|lvl| {
        let size_str = &lvl.remaining_base_amount;
        if size_str == "0" || size_str == "0.0" || size_str == "0.00" || size_str.is_empty() {
            return None;
        }
        let size: f64 = size_str.parse().ok()?;
        if size > 0.0 {
            lvl.price.parse::<f64>().ok()
        } else {
            None
        }
    })?;

    Some((best_bid, best_ask))
}

fn best_from_event(event: &OrderBookEvent) -> Option<(f64, f64)> {
    // Find best bid - skip orders with zero size (cancelled/filled)
    let bid = event
        .state
        .bids
        .iter()
        .find_map(|lvl| {
            let size_str = lvl.remaining_base_amount.as_deref().unwrap_or(&lvl.size);
            // Skip zero sizes (use same logic as synthetic BBO)
            if size_str == "0" || size_str == "0.0" || size_str == "0.00" || size_str.is_empty() {
                return None;
            }
            let size: f64 = size_str.parse().ok()?;
            if size > 0.0 {
                lvl.price.parse::<f64>().ok()
            } else {
                None
            }
        })?;

    // Find best ask - skip orders with zero size
    let ask = event
        .state
        .asks
        .iter()
        .find_map(|lvl| {
            let size_str = lvl.remaining_base_amount.as_deref().unwrap_or(&lvl.size);
            // Skip zero sizes (use same logic as synthetic BBO)
            if size_str == "0" || size_str == "0.0" || size_str == "0.00" || size_str.is_empty() {
                return None;
            }
            let size: f64 = size_str.parse().ok()?;
            if size > 0.0 {
                lvl.price.parse::<f64>().ok()
            } else {
                None
            }
        })?;

    Some((bid, ask))
}

fn print_side(label: &str, prices: &[f64], order_size: f64) {
    for (idx, price) in prices.iter().enumerate() {
        println!(
            "   {:>2}. {:>3} {:.6} @ ${:.2}",
            idx + 1,
            label,
            order_size,
            price
        );
    }
}
