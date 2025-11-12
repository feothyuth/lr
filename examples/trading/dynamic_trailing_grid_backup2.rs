//! Multi-stream WebSocket diagnostics for the dynamic grid bot.
//!
//! This mirrors the channel layout from `mm_hawkes.rs`:
//! - Separate public sockets for order book, market stats, and public trades
//! - Dedicated authenticated sockets for account market orders/positions and fills
//! - Purely observational: no order placement or cancellation logic

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use chrono::Utc;
use common::{connect_private_stream, ExampleContext};
use futures_util::StreamExt;
use lighter_client::ws_client::{
    AccountEvent, AccountEventEnvelope, OrderBookEvent, TradeData, WsEvent,
};
use serde_json::Value;
use std::{
    collections::HashSet,
    io::{self, Write},
    time::{Duration, Instant},
};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let ctx = ExampleContext::initialise(Some("dynamic_trailing_grid_monitor")).await?;
    let market = ctx.market_id();
    let account = ctx.account_id();

    println!("\n{}", "═".repeat(88));
    println!(
        "🔍 Feed Monitor | Market {} | Account {}",
        market.into_inner(),
        account.into_inner()
    );
    println!("{}", "═".repeat(88));

    // Public sockets
    let mut order_stream = ctx
        .ws_builder()
        .subscribe_order_book(market)
        .connect()
        .await?;
    println!("✅ Order-book stream connected");

    let mut stats_stream = ctx
        .ws_builder()
        .subscribe_market_stats(market)
        .connect()
        .await?;
    println!("✅ Market-stats stream connected");

    let mut trades_stream = ctx.ws_builder().subscribe_trade(market).connect().await?;
    println!("✅ Public trades stream connected");

    // Account sockets (per-market plus global snapshot)
    let orders_builder = ctx
        .ws_builder()
        .subscribe_account_market_orders(market, account)
        .subscribe_account_market_positions(market, account);
    let (mut account_orders_stream, _) = connect_private_stream(&ctx, orders_builder).await?;
    println!("✅ Account market orders/positions stream connected");

    let trades_builder = ctx
        .ws_builder()
        .subscribe_account_market_trades(market, account);
    let (mut account_trades_stream, _) = connect_private_stream(&ctx, trades_builder).await?;
    println!("✅ Account market fills stream connected\n");

    let all_builder = ctx
        .ws_builder()
        .subscribe_account_all_orders(account)
        .subscribe_account_all_positions(account);
    let (mut account_all_stream, _) = connect_private_stream(&ctx, all_builder).await?;
    println!("✅ Account all-orders/all-positions stream connected\n");

    let mut state = FeedState::default();

    loop {
        tokio::select! {
            evt = order_stream.next() => {
                if !handle_order_stream(&mut state, evt) { break; }
            }
            evt = stats_stream.next() => {
                if !handle_stats_stream(&mut state, evt) { break; }
            }
            evt = trades_stream.next() => {
                if !handle_public_trade_stream(&mut state, evt) { break; }
            }
            evt = account_orders_stream.next() => {
                if !handle_account_stream(&mut state, evt, AccountStreamKind::MarketOrdersPositions(market.into_inner())) { break; }
            }
            evt = account_trades_stream.next() => {
                if !handle_account_stream(&mut state, evt, AccountStreamKind::Fills) { break; }
            }
            evt = account_all_stream.next() => {
                if !handle_account_stream(&mut state, evt, AccountStreamKind::AllSnapshot) { break; }
            }
        }
    }

    Ok(())
}

fn handle_order_stream(
    state: &mut FeedState,
    event: Option<Result<WsEvent, lighter_client::errors::WsClientError>>,
) -> bool {
    match event {
        Some(Ok(WsEvent::OrderBook(book))) => {
            log_arrival("ORDER_BOOK");
            state.update_book(&book);
            state.render();
            true
        }
        Some(Ok(WsEvent::Connected)) | Some(Ok(WsEvent::Pong)) => true,
        Some(Ok(other)) => {
            println!("ℹ️  Order stream event: {:?}", other);
            true
        }
        Some(Err(err)) => {
            eprintln!("❌ Order stream error: {err}");
            false
        }
        None => {
            println!("🔌 Order stream closed");
            false
        }
    }
}

fn handle_stats_stream(
    state: &mut FeedState,
    event: Option<Result<WsEvent, lighter_client::errors::WsClientError>>,
) -> bool {
    match event {
        Some(Ok(WsEvent::MarketStats(stats))) => {
            log_arrival("MARKET_STATS");
            state.update_stats(
                stats.market_stats.mark_price.parse().ok(),
                stats.market_stats.index_price.parse().ok(),
                Some(stats.market_stats.daily_price_change),
            );
            state.render();
            true
        }
        Some(Ok(WsEvent::Connected)) | Some(Ok(WsEvent::Pong)) => true,
        Some(Ok(other)) => {
            println!("ℹ️  Market-stats stream event: {:?}", other);
            true
        }
        Some(Err(err)) => {
            eprintln!("❌ Market-stats stream error: {err}");
            false
        }
        None => {
            println!("🔌 Market-stats stream closed");
            false
        }
    }
}

fn handle_public_trade_stream(
    state: &mut FeedState,
    event: Option<Result<WsEvent, lighter_client::errors::WsClientError>>,
) -> bool {
    match event {
        Some(Ok(WsEvent::Trade(trade_event))) => {
            log_arrival("PUBLIC_TRADE");
            state.update_public_trade(&trade_event.trades);
            state.render();
            true
        }
        Some(Ok(WsEvent::Connected)) | Some(Ok(WsEvent::Pong)) => true,
        Some(Ok(other)) => {
            println!("ℹ️  Public trade stream event: {:?}", other);
            true
        }
        Some(Err(err)) => {
            eprintln!("❌ Public trade stream error: {err}");
            false
        }
        None => {
            println!("🔌 Public trade stream closed");
            false
        }
    }
}

fn handle_account_stream(
    state: &mut FeedState,
    event: Option<Result<WsEvent, lighter_client::errors::WsClientError>>,
    kind: AccountStreamKind,
) -> bool {
    match event {
        Some(Ok(WsEvent::Account(envelope))) => {
            log_arrival(kind.label());
            let envelope = match kind {
                AccountStreamKind::MarketOrdersPositions(market_id) => {
                    normalise_orders_positions(envelope, market_id)
                }
                AccountStreamKind::AllSnapshot => envelope,
                AccountStreamKind::Fills => envelope,
            };
            state.update_account(envelope);
            state.render();
            true
        }
        Some(Ok(WsEvent::Connected)) | Some(Ok(WsEvent::Pong)) => true,
        Some(Ok(other)) => {
            println!("ℹ️  Account stream ({}) event: {:?}", kind.label(), other);
            true
        }
        Some(Err(err)) => {
            eprintln!("❌ Account stream ({}) error: {err}", kind.label());
            false
        }
        None => {
            println!("🔌 Account stream ({}) closed", kind.label());
            false
        }
    }
}

struct FeedState {
    order_book: Option<BookSummary>,
    market_stats: Option<StatsSummary>,
    last_public_trade: Option<TradeSummary>,
    last_account_trade: Option<TradeSummary>,
    positions: Option<String>,
    orders: Option<String>,
    last_render: Instant,
}

impl FeedState {
    fn update_book(&mut self, event: &OrderBookEvent) {
        if let (Some(bid), Some(ask)) = (
            event
                .state
                .bids
                .first()
                .and_then(|lvl| lvl.price.parse::<f64>().ok()),
            event
                .state
                .asks
                .first()
                .and_then(|lvl| lvl.price.parse::<f64>().ok()),
        ) {
            self.order_book = Some(BookSummary { bid, ask });
        }
    }

    fn update_stats(&mut self, mark: Option<f64>, index: Option<f64>, change: Option<f64>) {
        self.market_stats = Some(StatsSummary {
            mark,
            index,
            change,
        });
    }

    fn update_public_trade(&mut self, trades: &[TradeData]) {
        if let Some(trade) = trades.last() {
            let size = if !trade.base_size.is_empty() {
                trade.base_size.clone()
            } else {
                trade.quote_size.clone()
            };
            self.last_public_trade = Some(TradeSummary {
                price: trade.price.clone(),
                size,
                side: trade.side.clone(),
            });
        }
    }

    fn update_account(&mut self, envelope: AccountEventEnvelope) {
        let value = envelope.event.as_value();

        if let Some(trades) = value.get("trades").and_then(|v| v.as_array()) {
            if let Some(summary) = trades.iter().rev().find_map(summarise_account_trade) {
                self.last_account_trade = Some(summary);
            }
        }

        if let Some(orders) = value.get("orders") {
            self.orders = summarise_orders(orders);
        }

        if let Some(positions) = value.get("positions") {
            self.positions = summarise_positions(positions);
        }
    }

    fn render(&mut self) {
        if self.last_render.elapsed() < Duration::from_millis(150) {
            return;
        }
        self.last_render = Instant::now();

        print!("\x1B[2J\x1B[H");
        io::stdout().flush().ok();
        println!("{}", "═".repeat(88));
        println!(
            "🖥️  Feed Monitor @ {}",
            Utc::now().format("%Y-%m-%d %H:%M:%S%.3f")
        );
        println!("{}", "═".repeat(88));

        if let Some(book) = &self.order_book {
            println!(
                "Order Book : bid={:.2} ask={:.2} spread={:.2}",
                book.bid,
                book.ask,
                book.ask - book.bid
            );
        } else {
            println!("Order Book : n/a");
        }

        if let Some(stats) = &self.market_stats {
            println!(
                "Stats      : mark={:.2?} index={:.2?} Δ24h={:.2?}%",
                stats.mark, stats.index, stats.change
            );
        } else {
            println!("Stats      : n/a");
        }

        match (&self.last_public_trade, &self.last_account_trade) {
            (Some(pub_trade), Some(acc_trade)) => println!(
                "Trades     : {} | {}",
                pub_trade.display_with_label("PUBLIC"),
                acc_trade.display_with_label("ACCOUNT")
            ),
            (Some(pub_trade), None) => {
                println!("Trades     : {}", pub_trade.display_with_label("PUBLIC"))
            }
            (None, Some(acc_trade)) => {
                println!("Trades     : {}", acc_trade.display_with_label("ACCOUNT"))
            }
            _ => println!("Trades     : n/a"),
        }

        println!(
            "Positions  : {}",
            self.positions.as_deref().unwrap_or("n/a")
        );
        println!("Orders     : {}", self.orders.as_deref().unwrap_or("n/a"));
    }
}

#[derive(Clone, Copy)]
struct BookSummary {
    bid: f64,
    ask: f64,
}

#[derive(Clone, Copy)]
struct StatsSummary {
    mark: Option<f64>,
    index: Option<f64>,
    change: Option<f64>,
}

#[derive(Clone)]
struct TradeSummary {
    price: String,
    size: String,
    side: String,
}

impl TradeSummary {
    fn display_with_label(&self, label: &str) -> String {
        format!(
            "{} {} {} @ {}",
            label,
            self.side.to_ascii_uppercase(),
            self.size,
            self.price
        )
    }
}

impl Default for FeedState {
    fn default() -> Self {
        Self {
            order_book: None,
            market_stats: None,
            last_public_trade: None,
            last_account_trade: None,
            positions: None,
            orders: None,
            last_render: Instant::now() - Duration::from_secs(1),
        }
    }
}

fn summarise_account_trade(trade: &Value) -> Option<TradeSummary> {
    let price = extract_value(trade, "price")?;
    let size = extract_value(trade, "size")
        .or_else(|| extract_value(trade, "base_size"))
        .or_else(|| extract_value(trade, "quote_size"))
        .unwrap_or_else(|| "?".to_string());
    let side = trade
        .get("side")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    Some(TradeSummary {
        price,
        size,
        side: side.to_string(),
    })
}

fn summarise_positions(value: &Value) -> Option<String> {
    let mut entries = Vec::new();

    if let Some(obj) = value.as_object() {
        for (market, positions) in obj.iter() {
            if let Some(array) = positions.as_array() {
                collect_positions(&mut entries, market, array);
            } else if positions.is_object() {
                collect_positions(&mut entries, market, std::slice::from_ref(positions));
            }
            if entries.len() >= 4 {
                break;
            }
        }
    } else if let Some(array) = value.as_array() {
        collect_positions(&mut entries, "", array);
    }

    if entries.is_empty() {
        None
    } else {
        Some(entries.join(" | "))
    }
}

fn collect_positions(entries: &mut Vec<String>, market_key: &str, array: &[Value]) {
    for position in array {
        let qty = extract_value(position, "position")
            .or_else(|| extract_value(position, "size"))
            .unwrap_or_else(|| "0".to_string());
        if qty == "0" {
            continue;
        }
        let side = position.get("side").and_then(|v| v.as_str()).unwrap_or("");
        let symbol = position
            .get("symbol")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(market_key);
        let display = if symbol.is_empty() {
            market_key
        } else {
            symbol
        };
        entries.push(format!("{}:{}{}", display, qty, format_side(side)));
        if entries.len() >= 4 {
            break;
        }
    }
}

fn format_side(side: &str) -> String {
    match side.to_ascii_lowercase().as_str() {
        "buy" | "long" => "↑".to_string(),
        "sell" | "short" => "↓".to_string(),
        _ => "".to_string(),
    }
}

fn summarise_orders(value: &Value) -> Option<String> {
    let obj = value.as_object()?;
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut seen_side = HashSet::new();

    'outer: for (market, orders) in obj.iter() {
        if let Some(array) = orders.as_array() {
            for order in array {
                let idx = order
                    .get("order_index")
                    .and_then(|v| v.as_i64())
                    .or_else(|| order.get("client_order_index").and_then(|v| v.as_i64()))
                    .unwrap_or(-1);
                if idx >= 0 && !seen.insert(idx) {
                    continue;
                }

                let side = if order
                    .get("is_ask")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    "ASK"
                } else {
                    "BID"
                };
                let price = extract_value(order, "price").unwrap_or_else(|| "?".to_string());
                let remaining = extract_value(order, "remaining_base_amount")
                    .or_else(|| extract_value(order, "size"))
                    .unwrap_or_else(|| "?".to_string());
                entries.push(format!("{}:{} {} @ {}", market, side, remaining, price));
                seen_side.insert(side.to_string());

                if seen_side.len() == 2 && entries.len() >= 2 {
                    break 'outer;
                }
                if entries.len() >= 12 {
                    break 'outer;
                }
            }
        }
    }

    if entries.is_empty() {
        None
    } else {
        Some(entries.join(" | "))
    }
}

fn extract_value(map: &Value, key: &str) -> Option<String> {
    map.get(key).and_then(|value| {
        value
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| value.as_f64().map(|f| format!("{:.6}", f)))
    })
}

fn log_arrival(label: &str) {
    println!(
        "[{}] {} update received",
        Utc::now().format("%H:%M:%S%.3f"),
        label
    );
}
enum AccountStreamKind {
    MarketOrdersPositions(i32),
    AllSnapshot,
    Fills,
}

impl AccountStreamKind {
    fn label(&self) -> &'static str {
        match self {
            AccountStreamKind::MarketOrdersPositions(_) => "ACCOUNT_MARKET_ORDERS_POSITIONS",
            AccountStreamKind::AllSnapshot => "ACCOUNT_ALL_SNAPSHOT",
            AccountStreamKind::Fills => "ACCOUNT_FILLS",
        }
    }
}

fn normalise_orders_positions(
    mut envelope: AccountEventEnvelope,
    market_id: i32,
) -> AccountEventEnvelope {
    let mut value = envelope.event.as_value().clone();
    if let Some(map) = value.as_object_mut() {
        promote_map_entry(map, "positions", market_id);
        promote_map_entry(map, "orders", market_id);
    }
    envelope.event = AccountEvent::new(value);
    envelope
}

fn promote_map_entry(map: &mut serde_json::Map<String, Value>, key: &str, market_id: i32) {
    if let Some(entry) = map.get_mut(key) {
        if entry.is_array() {
            let mut new_map = serde_json::Map::new();
            new_map.insert(market_id.to_string(), entry.clone());
            *entry = Value::Object(new_map);
        }
    }
}
