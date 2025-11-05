//! Autonomous order manager that places IOC bids, monitors account updates over
//! WebSocket, and cancels all orders once the active count exceeds a threshold.
//!
//! This is a condensed version of the original `examples/test_order` crate
//! converted into a standard `cargo run --example` target.
//! It showcases how to:
//!   - Fetch order book data before quoting.
//!   - Submit Immediate-Or-Cancel orders through the high-level SDK.
//!   - Track active orders via REST and cached state.
//!   - Consume both public (order book) and private (account) WebSocket streams.
//!   - Automatically cancel all orders when inventory grows too large.
//!
//! Required environment variables:
//!   * `LIGHTER_PRIVATE_KEY`
//!   * `LIGHTER_ACCOUNT_INDEX`
//!   * `LIGHTER_API_KEY_INDEX`
//! Optional:
//!   * `LIGHTER_API_URL` (defaults to mainnet)
//!   * `LIGHTER_MARKET_ID` (defaults to 0 / BTC-PERP)

use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex, BaseQty, MarketId, Price},
    ws_client::{OrderBookEvent, WsEvent},
};
use std::collections::HashMap;
use std::num::NonZeroI64;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::{interval, sleep};

const MAX_ACTIVE_ORDERS: usize = 9;
const ORDER_INTERVAL_SECS: u64 = 5;
const ACTIVE_CHECK_INTERVAL_SECS: u64 = 2;

#[derive(Clone)]
struct OrderManager {
    active_orders: Arc<RwLock<HashMap<String, OrderInfo>>>,
    client: Arc<LighterClient>,
    market_id: MarketId,
    account_id: AccountId,
}

#[derive(Debug, Clone)]
struct OrderInfo {
    order_id: String,
    market_index: i32,
    base_amount: String,
    price: String,
    is_ask: bool,
    created_at: std::time::Instant,
}

impl OrderManager {
    fn new(client: LighterClient, market_id: MarketId, account_id: AccountId) -> Self {
        Self {
            active_orders: Arc::new(RwLock::new(HashMap::new())),
            client: Arc::new(client),
            market_id,
            account_id,
        }
    }

    async fn create_single_order(&self) -> anyhow::Result<()> {
        println!("🔄 Creating new order...");

        let orderbook = self.client.orders().book(self.market_id, 10).await?;
        let (price, is_ask) = if let Some(best_bid) = orderbook.bids.first() {
            (best_bid.price.parse::<f64>().unwrap_or(0.0) * 0.999, false)
        } else if let Some(best_ask) = orderbook.asks.first() {
            (best_ask.price.parse::<f64>().unwrap_or(0.0) * 1.001, true)
        } else {
            (50000.0, false)
        };

        let builder = if is_ask {
            self.client.order(self.market_id).sell()
        } else {
            self.client.order(self.market_id).buy()
        };

        let price_ticks = Price::ticks((price * 100.0).round() as i64);
        let base_amount = BaseQty::new(NonZeroI64::new(100).unwrap()); // 0.01 units assuming 4 decimals

        let submission = builder
            .qty(base_amount)
            .limit(price_ticks)
            .ioc()
            .auto_client_id()
            .submit()
            .await?;

        println!("✅ Order submitted: {}", submission.response().code);
        if let Some(message) = &submission.response().message {
            if !message.is_empty() {
                println!("   Message: {}", message);
            }
        }
        let tx_hash = &submission.response().tx_hash;
        if !tx_hash.is_empty() {
            println!("📝 Transaction hash: {}", tx_hash);
        }

        Ok(())
    }

    async fn refresh_active_orders(&self) -> anyhow::Result<usize> {
        let orders = self.client.account().active_orders(self.market_id).await?;
        let count = orders.orders.len();

        let mut active_orders = self.active_orders.write().await;
        active_orders.clear();

        for order in &orders.orders {
            active_orders.insert(
                order.order_id.clone(),
                OrderInfo {
                    order_id: order.order_id.clone(),
                    market_index: order.market_index,
                    base_amount: order.initial_base_amount.clone(),
                    price: order.price.clone(),
                    is_ask: order.is_ask,
                    created_at: std::time::Instant::now(),
                },
            );
        }

        println!("📊 Current active orders: {}", count);
        Ok(count)
    }

    async fn cancel_all_orders(&self) -> anyhow::Result<()> {
        println!("🚨 Canceling all orders (>{} active)", MAX_ACTIVE_ORDERS);
        let submission = self.client.cancel_all().submit().await?;

        println!("✅ Cancel-all response: {}", submission.response().code);
        let tx_hash = &submission.response().tx_hash;
        if !tx_hash.is_empty() {
            println!("📝 Cancel transaction hash: {}", tx_hash);
        }

        let mut active_orders = self.active_orders.write().await;
        active_orders.clear();
        Ok(())
    }

    async fn start_websocket_monitoring(&self) -> anyhow::Result<()> {
        println!("🔌 Starting WebSocket monitoring...");

        let mut builder = self.client.ws();
        builder = builder.subscribe_order_book(self.market_id);
        builder = builder.subscribe_account(self.account_id);

        let auth_token = self
            .client
            .signer()
            .and_then(|signer| signer.create_auth_token_with_expiry(None).ok())
            .map(|token| token.token);

        let mut stream = builder.connect().await?;
        if let Some(token) = auth_token {
            stream.connection_mut().set_auth_token(token);
        }

        while let Some(event) = stream.next().await {
            match event {
                Ok(WsEvent::Connected) => println!("✅ WebSocket connected"),
                Ok(WsEvent::Pong) => println!("🏓 Heartbeat pong"),
                Ok(WsEvent::OrderBook(orderbook_event)) => {
                    self.handle_orderbook_update(orderbook_event).await;
                }
                Ok(WsEvent::Account(envelope)) => {
                    self.handle_account_update(envelope.event.as_value().clone())
                        .await;
                }
                Ok(other) => println!("ℹ️  WS event: {:?}", other),
                Err(err) => {
                    println!("❌ WebSocket error: {:?}", err);
                    break;
                }
            }
        }

        println!("🔌 WebSocket monitoring stopped");
        Ok(())
    }

    async fn handle_orderbook_update(&self, event: OrderBookEvent) {
        println!(
            "📊 Order book update for market {}: {} bids / {} asks",
            event.market.into_inner(),
            event.state.bids.len(),
            event.state.asks.len()
        );
    }

    async fn handle_account_update(&self, payload: serde_json::Value) {
        if let Some(event_type) = payload.get("type").and_then(|v| v.as_str()) {
            println!("📢 Account event: {}", event_type);
            if matches!(
                event_type,
                "OrderFilled" | "OrderCancelled" | "OrderCreated"
            ) {
                if let Err(err) = self.refresh_active_orders().await {
                    println!(
                        "⚠️  Failed to refresh orders after account event: {:?}",
                        err
                    );
                }
            }
        } else {
            println!("📢 Account update: {:?}", payload);
        }
    }

    async fn run_order_management_loop(&self) -> anyhow::Result<()> {
        let mut order_interval = interval(Duration::from_secs(ORDER_INTERVAL_SECS));
        let mut check_interval = interval(Duration::from_secs(ACTIVE_CHECK_INTERVAL_SECS));

        loop {
            tokio::select! {
                _ = order_interval.tick() => {
                    if let Err(err) = self.create_single_order().await {
                        println!("❌ Failed to create order: {:?}", err);
                    }
                }
                _ = check_interval.tick() => {
                    match self.refresh_active_orders().await {
                        Ok(count) if count > MAX_ACTIVE_ORDERS => {
                            if let Err(err) = self.cancel_all_orders().await {
                                println!("❌ Failed to cancel all orders: {:?}", err);
                            }
                        }
                        Ok(_) => {}
                        Err(err) => println!("❌ Failed to fetch active orders: {:?}", err),
                    }
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🚀 Autonomous Order Manager");

    let api_url = std::env::var("LIGHTER_API_URL")
        .unwrap_or_else(|_| "https://mainnet.zklighter.elliot.ai".to_string());
    let private_key = env_or_die("LIGHTER_PRIVATE_KEY");
    let account_index: i64 = env_or_die("LIGHTER_ACCOUNT_INDEX").parse()?;
    let api_key_index: i32 = env_or_die("LIGHTER_API_KEY_INDEX").parse()?;
    let market_id: i32 = std::env::var("LIGHTER_MARKET_ID")
        .unwrap_or_else(|_| "0".to_string())
        .parse()?;

    println!("🔧 Configuration:");
    println!("  API URL: {}", api_url);
    println!("  Account Index: {}", account_index);
    println!("  API Key Index: {}", api_key_index);
    println!("  Market ID: {}", market_id);
    println!("  Max active orders before cancel: {}", MAX_ACTIVE_ORDERS);
    println!(
        "  Order interval: {}s | Active check: {}s",
        ORDER_INTERVAL_SECS, ACTIVE_CHECK_INTERVAL_SECS
    );
    println!();

    let client = LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key)
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    let details = client.account().details().await?;
    if let Some(account) = details.accounts.first() {
        println!("👤 Account summary:");
        println!("  Total orders historically: {}", account.total_order_count);
        println!("  Open positions: {}", account.positions.len());
    }

    let manager = OrderManager::new(
        client,
        MarketId::new(market_id),
        AccountId::new(account_index),
    );

    let ws_manager = manager.clone();
    let ws_handle = tokio::spawn(async move {
        if let Err(err) = ws_manager.start_websocket_monitoring().await {
            println!("❌ WebSocket task exited with error: {:?}", err);
        }
    });

    sleep(Duration::from_secs(2)).await;

    let order_handle = tokio::spawn(async move {
        if let Err(err) = manager.run_order_management_loop().await {
            println!("❌ Order loop exited with error: {:?}", err);
        }
    });

    tokio::select! {
        _ = ws_handle => println!("🔌 WebSocket task finished"),
        _ = order_handle => println!("🎯 Order management loop finished"),
    };

    Ok(())
}

fn env_or_die(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("set the {} environment variable", key))
}
