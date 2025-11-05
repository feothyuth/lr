use std::{
    collections::HashMap,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use futures_util::{FutureExt, SinkExt, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};
use url::Url;

use crate::{
    errors::{WsClientError, WsResult},
    lighter_client::LighterClient,
    types::{AccountId, MarketId},
};

#[derive(Debug, Clone)]
pub struct ExponentialBackoff {
    pub initial: Duration,
    pub max: Duration,
    pub multiplier: f64,
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(500),
            max: Duration::from_secs(30),
            multiplier: 2.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WsConfig {
    pub host: String,
    pub path: String,
    pub backoff: ExponentialBackoff,
}

const SUPPRESS_ALREADY_SUB_DURATION: Duration = Duration::from_secs(2);

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            path: "/stream".to_string(),
            backoff: ExponentialBackoff::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SubscriptionSet {
    pub order_books: Vec<MarketId>,
    pub accounts: Vec<AccountId>,
    // Public channels
    pub bbo: Vec<MarketId>,
    pub trades: Vec<MarketId>,
    pub market_stats: Vec<MarketId>,
    pub subscribe_transactions: bool,
    pub subscribe_executed_transactions: bool,
    pub subscribe_height: bool,
    // Private channels (per account)
    pub account_all_positions: Vec<AccountId>,
    pub account_all_trades: Vec<AccountId>,
    pub account_all_orders: Vec<AccountId>,
    pub user_stats: Vec<AccountId>,
    pub account_tx: Vec<AccountId>,
    pub pool_data: Vec<AccountId>,
    pub pool_info: Vec<AccountId>,
    pub notifications: Vec<AccountId>,
    // Market-specific private channels (market, account)
    pub account_market: Vec<(MarketId, AccountId)>,
    pub account_market_orders: Vec<(MarketId, AccountId)>,
    pub account_market_positions: Vec<(MarketId, AccountId)>,
    pub account_market_trades: Vec<(MarketId, AccountId)>,
}

impl SubscriptionSet {
    pub fn add_order_book(&mut self, market: MarketId) {
        self.order_books.push(market);
    }

    pub fn add_account(&mut self, account: AccountId) {
        self.accounts.push(account);
    }

    pub fn extend_order_books<I: IntoIterator<Item = MarketId>>(&mut self, markets: I) {
        self.order_books.extend(markets);
    }

    pub fn add_bbo(&mut self, market: MarketId) {
        self.bbo.push(market);
    }

    pub fn add_trade(&mut self, market: MarketId) {
        self.trades.push(market);
    }

    pub fn add_market_stats(&mut self, market: MarketId) {
        self.market_stats.push(market);
    }

    pub fn add_account_all_positions(&mut self, account: AccountId) {
        self.account_all_positions.push(account);
    }

    pub fn add_account_all_trades(&mut self, account: AccountId) {
        self.account_all_trades.push(account);
    }

    pub fn add_account_all_orders(&mut self, account: AccountId) {
        self.account_all_orders.push(account);
    }

    pub fn add_user_stats(&mut self, account: AccountId) {
        self.user_stats.push(account);
    }

    pub fn add_account_tx(&mut self, account: AccountId) {
        self.account_tx.push(account);
    }

    pub fn add_account_market(&mut self, market: MarketId, account: AccountId) {
        self.account_market.push((market, account));
    }

    pub fn add_account_market_orders(&mut self, market: MarketId, account: AccountId) {
        self.account_market_orders.push((market, account));
    }

    pub fn add_account_market_positions(&mut self, market: MarketId, account: AccountId) {
        self.account_market_positions.push((market, account));
    }

    pub fn add_account_market_trades(&mut self, market: MarketId, account: AccountId) {
        self.account_market_trades.push((market, account));
    }

    pub fn is_empty(&self) -> bool {
        self.order_books.is_empty()
            && self.accounts.is_empty()
            && self.bbo.is_empty()
            && self.trades.is_empty()
            && self.market_stats.is_empty()
            && !self.subscribe_transactions
            && !self.subscribe_executed_transactions
            && !self.subscribe_height
            && self.account_all_positions.is_empty()
            && self.account_all_trades.is_empty()
            && self.account_all_orders.is_empty()
            && self.user_stats.is_empty()
            && self.account_tx.is_empty()
            && self.pool_data.is_empty()
            && self.pool_info.is_empty()
            && self.notifications.is_empty()
            && self.account_market.is_empty()
            && self.account_market_orders.is_empty()
            && self.account_market_positions.is_empty()
            && self.account_market_trades.is_empty()
    }

    pub fn requires_auth(&self) -> bool {
        !self.accounts.is_empty()
            || !self.account_all_positions.is_empty()
            || !self.account_all_trades.is_empty()
            || !self.account_all_orders.is_empty()
            || !self.user_stats.is_empty()
            || !self.account_tx.is_empty()
            || !self.pool_data.is_empty()
            || !self.pool_info.is_empty()
            || !self.notifications.is_empty()
            || !self.account_market.is_empty()
            || !self.account_market_orders.is_empty()
            || !self.account_market_positions.is_empty()
            || !self.account_market_trades.is_empty()
    }
}

pub struct WsBuilder<'a> {
    client: &'a LighterClient,
    subscriptions: SubscriptionSet,
    config: WsConfig,
}

impl<'a> WsBuilder<'a> {
    pub(crate) fn new(client: &'a LighterClient) -> Self {
        let mut config = client.websocket_config().clone();
        if config.host.is_empty() {
            config.host = client.rest_base_path().to_string();
        }
        Self {
            client,
            subscriptions: SubscriptionSet::default(),
            config,
        }
    }

    pub fn subscribe_order_book(mut self, market: MarketId) -> Self {
        self.subscriptions.add_order_book(market);
        self
    }

    pub fn subscribe_order_books<I: IntoIterator<Item = MarketId>>(mut self, markets: I) -> Self {
        self.subscriptions.extend_order_books(markets);
        self
    }

    pub fn subscribe_account(mut self, account: AccountId) -> Self {
        self.subscriptions.add_account(account);
        self
    }

    // Public channel subscriptions
    pub fn subscribe_bbo(mut self, market: MarketId) -> Self {
        self.subscriptions.add_bbo(market);
        self
    }

    pub fn subscribe_trade(mut self, market: MarketId) -> Self {
        self.subscriptions.add_trade(market);
        self
    }

    pub fn subscribe_market_stats(mut self, market: MarketId) -> Self {
        self.subscriptions.add_market_stats(market);
        self
    }

    pub fn subscribe_transactions(mut self) -> Self {
        self.subscriptions.subscribe_transactions = true;
        self
    }

    pub fn subscribe_executed_transactions(mut self) -> Self {
        self.subscriptions.subscribe_executed_transactions = true;
        self
    }

    pub fn subscribe_height(mut self) -> Self {
        self.subscriptions.subscribe_height = true;
        self
    }

    // Private channel subscriptions
    pub fn subscribe_account_all_positions(mut self, account: AccountId) -> Self {
        self.subscriptions.add_account_all_positions(account);
        self
    }

    pub fn subscribe_account_all_trades(mut self, account: AccountId) -> Self {
        self.subscriptions.add_account_all_trades(account);
        self
    }

    pub fn subscribe_account_all_orders(mut self, account: AccountId) -> Self {
        self.subscriptions.add_account_all_orders(account);
        self
    }

    pub fn subscribe_user_stats(mut self, account: AccountId) -> Self {
        self.subscriptions.add_user_stats(account);
        self
    }

    pub fn subscribe_account_tx(mut self, account: AccountId) -> Self {
        self.subscriptions.add_account_tx(account);
        self
    }

    pub fn subscribe_account_market(mut self, market: MarketId, account: AccountId) -> Self {
        self.subscriptions.add_account_market(market, account);
        self
    }

    pub fn subscribe_pool_data(mut self, account: AccountId) -> Self {
        self.subscriptions.pool_data.push(account);
        self
    }

    pub fn subscribe_pool_info(mut self, account: AccountId) -> Self {
        self.subscriptions.pool_info.push(account);
        self
    }

    pub fn subscribe_notification(mut self, account: AccountId) -> Self {
        self.subscriptions.notifications.push(account);
        self
    }

    // Market-specific private channels
    pub fn subscribe_account_market_orders(mut self, market: MarketId, account: AccountId) -> Self {
        self.subscriptions
            .add_account_market_orders(market, account);
        self
    }

    pub fn subscribe_account_market_positions(
        mut self,
        market: MarketId,
        account: AccountId,
    ) -> Self {
        self.subscriptions
            .add_account_market_positions(market, account);
        self
    }

    pub fn subscribe_account_market_trades(mut self, market: MarketId, account: AccountId) -> Self {
        self.subscriptions
            .add_account_market_trades(market, account);
        self
    }

    // Helper methods
    pub fn subscribe_all_public(mut self, market: MarketId) -> Self {
        self.subscriptions.add_order_book(market);
        self.subscriptions.add_bbo(market);
        self.subscriptions.add_trade(market);
        self.subscriptions.add_market_stats(market);
        self
    }

    pub fn subscribe_all_private(mut self, account: AccountId) -> Self {
        self.subscriptions.add_account(account);
        self.subscriptions.add_account_all_positions(account);
        self.subscriptions.add_account_all_trades(account);
        self.subscriptions.add_account_all_orders(account);
        self.subscriptions.add_user_stats(account);
        self.subscriptions.add_account_tx(account);
        self
    }

    pub fn subscribe_all(self, market: MarketId, account: AccountId) -> Self {
        self.subscribe_all_public(market)
            .subscribe_all_private(account)
            .subscribe_account_market(market, account)
    }

    pub fn backoff(mut self, backoff: ExponentialBackoff) -> Self {
        self.config.backoff = backoff;
        self
    }

    pub fn build(self) -> WsResult<WsClient> {
        WsClient::new(self.client, self.config, self.subscriptions)
    }

    pub async fn connect(self) -> WsResult<WsStream> {
        let WsBuilder {
            client,
            subscriptions,
            config,
        } = self;

        let auth_token = if subscriptions.requires_auth() {
            Some(
                client
                    .create_auth_token(None)
                    .map_err(|err| WsClientError::Auth(err.to_string()))?,
            )
        } else {
            None
        };

        let ws_client = WsClient::new(client, config, subscriptions)?;
        let mut connection = ws_client.connect().await?;

        if let Some(token) = auth_token {
            connection.set_auth_token(token);
        }

        Ok(WsStream::new(connection))
    }
}

#[derive(Debug, Clone)]
pub struct WsClient {
    config: WsConfig,
    subscriptions: SubscriptionSet,
    url: Url,
}

impl WsClient {
    fn new(
        client: &LighterClient,
        mut config: WsConfig,
        subscriptions: SubscriptionSet,
    ) -> WsResult<Self> {
        if subscriptions.is_empty() {
            return Err(WsClientError::EmptySubscriptions);
        }

        if config.host.is_empty() {
            config.host = client.rest_base_path().to_string();
        }

        let url = build_url(&config)?;
        Ok(Self {
            config,
            subscriptions,
            url,
        })
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn subscriptions(&self) -> &SubscriptionSet {
        &self.subscriptions
    }

    pub async fn connect(self) -> WsResult<WsConnection> {
        let (stream, _) = connect_async(self.url.as_str()).await?;
        Ok(WsConnection::new(
            self.url,
            stream,
            self.subscriptions,
            self.config.backoff,
        ))
    }
}

#[derive(Debug)]
pub struct WsConnection {
    url: Url,
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    subscriptions: SubscriptionSet,
    state: WsState,
    backoff: ExponentialBackoff,
    auth_token: Option<String>,
    message_id: u64,
    generation: u64,
    suppress_already_subscribed_until: Option<Instant>,
}

#[derive(Debug, Default)]
struct WsState {
    order_books: HashMap<MarketId, OrderBookState>,
    order_book_offsets: HashMap<MarketId, i64>,
    accounts: HashMap<AccountId, AccountEvent>,
}

impl WsConnection {
    fn new(
        url: Url,
        stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
        subscriptions: SubscriptionSet,
        backoff: ExponentialBackoff,
    ) -> Self {
        Self {
            url,
            stream,
            subscriptions,
            state: WsState::default(),
            backoff,
            auth_token: None,
            message_id: 0,
            generation: 0,
            suppress_already_subscribed_until: Some(Instant::now() + SUPPRESS_ALREADY_SUB_DURATION),
        }
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn subscriptions(&self) -> &SubscriptionSet {
        &self.subscriptions
    }

    pub fn order_book_state(&self, market: MarketId) -> Option<&OrderBookState> {
        self.state.order_books.get(&market)
    }

    pub fn account_state(&self, account: AccountId) -> Option<&AccountEvent> {
        self.state.accounts.get(&account)
    }

    pub fn backoff(&self) -> &ExponentialBackoff {
        &self.backoff
    }

    pub fn suppressing_already_subscribed(&self) -> bool {
        self.suppress_already_subscribed_until
            .map(|until| Instant::now() <= until)
            .unwrap_or(false)
    }

    pub async fn unsubscribe_order_book_channel(&mut self, market: MarketId) -> WsResult<()> {
        self.subscriptions.order_books.retain(|m| *m != market);
        self.state.order_books.remove(&market);
        let payload = json!({
            "type": "unsubscribe",
            "channel": format!("order_book/{}", market.into_inner()),
        })
        .to_string();
        self.stream.send(Message::Text(payload)).await?;
        Ok(())
    }

    pub async fn subscribe_order_book_channel(&mut self, market: MarketId) -> WsResult<()> {
        if !self.subscriptions.order_books.iter().any(|m| *m == market) {
            self.subscriptions.order_books.push(market);
        }
        let payload = json!({
            "type": "subscribe",
            "channel": format!("order_book/{}", market.into_inner()),
        })
        .to_string();
        self.stream.send(Message::Text(payload)).await?;
        Ok(())
    }

    pub async fn resubscribe_order_book_channel(&mut self, market: MarketId) -> WsResult<()> {
        tracing::debug!(?market, "Resubscribing order_book channel in-place");
        self.unsubscribe_order_book_channel(market).await?;
        // give the server a short beat to process the unsubscribe to avoid duplicate sub errors
        tokio::time::sleep(Duration::from_millis(20)).await;
        self.subscribe_order_book_channel(market).await
    }

    pub async fn next_event(&mut self) -> WsResult<Option<WsEvent>> {
        use tokio::time::timeout;

        loop {
            // Add 30-second timeout to keep event loop responsive during quiet periods
            // This prevents WebSocket staleness by allowing the loop to continue even
            // when no messages are received, enabling proper ping/pong handling
            match timeout(Duration::from_secs(30), self.stream.next()).await {
                Ok(Some(Ok(message))) => {
                    // Got a message - process it normally
                    match message {
                        Message::Text(text) => {
                            return self.handle_text_message(text).await;
                        }
                        Message::Binary(binary) => {
                            let text = String::from_utf8(binary).map_err(|_| {
                                WsClientError::InvalidMessage("invalid utf8 payload".to_string())
                            })?;
                            return self.handle_text_message(text).await;
                        }
                        Message::Ping(payload) => {
                            tracing::info!("🏓 Received WebSocket PING, sending PONG");
                            self.stream.send(Message::Pong(payload)).await?;
                            return Ok(Some(WsEvent::Pong));
                        }
                        Message::Pong(_) => {
                            return Ok(Some(WsEvent::Pong));
                        }
                        Message::Close(frame) => {
                            let info = frame.map(|frame| CloseFrameInfo {
                                code: u16::from(frame.code),
                                reason: frame.reason.into_owned(),
                            });
                            return Ok(Some(WsEvent::Closed(info)));
                        }
                        Message::Frame(_) => {
                            // Ignore frame messages, continue loop
                        }
                    }
                }
                Ok(Some(Err(e))) => {
                    // WebSocket error
                    return Err(e.into());
                }
                Ok(None) => {
                    // Connection closed by server
                    return Ok(None);
                }
                Err(_) => {
                    // Timeout occurred - connection is quiet but OK
                    // Continue loop to check for messages again
                    // This prevents the blocking issue that causes staleness
                    tracing::debug!(
                        "WebSocket message read timeout (connection quiet, continuing...)"
                    );
                    continue;
                }
            }
        }
    }

    pub async fn close(mut self) -> WsResult<()> {
        self.stream.close(None).await?;
        Ok(())
    }

    /// Set authentication token for private channel subscriptions and transaction submission
    pub fn set_auth_token(&mut self, token: String) {
        self.auth_token = Some(token);
    }

    /// Get the current authentication token
    pub fn auth_token(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }

    async fn send_auth_if_needed(&mut self) -> WsResult<()> {
        // Currently transactions embed the auth token per message, so nothing to send here.
        Ok(())
    }

    /// Generate next message ID
    fn next_message_id(&mut self) -> u64 {
        self.message_id += 1;
        self.message_id
    }

    /// Send a single transaction via WebSocket
    ///
    /// # Arguments
    /// * `tx_type` - Transaction type (14=CreateOrder, 15=CancelOrder, etc.)
    /// * `tx_info` - Signed transaction data as JSON value
    ///
    /// # Returns
    /// Result indicating success or failure
    ///
    /// # Example
    /// ```ignore
    /// connection.send_transaction(14, tx_info).await?;
    /// ```
    pub async fn send_transaction(&mut self, tx_type: u8, tx_info: Value) -> WsResult<()> {
        let token = self.auth_token.clone().ok_or_else(|| {
            WsClientError::InvalidMessage("Authentication token not set".to_string())
        })?;

        let msg_id = self.next_message_id();

        let msg = json!({
            "type": "jsonapi/sendtx",
            "data": {
                "id": format!("tx_{}", msg_id),
                "tx_type": tx_type,
                "tx_info": tx_info,
                "token": token,
            }
        });

        self.stream.send(Message::Text(msg.to_string())).await?;

        Ok(())
    }

    /// Send multiple transactions in a single batch (max 50)
    ///
    /// # Arguments
    /// * `tx_types` - Vector of transaction types
    /// * `tx_infos` - Vector of signed transaction data
    ///
    /// # Returns
    /// Result indicating success or failure
    ///
    /// # Example
    /// ```ignore
    /// let tx_types = vec![14, 14, 15]; // Create, Create, Cancel
    /// let tx_infos = vec![order1, order2, cancel1];
    /// connection.send_batch_transaction(tx_types, tx_infos).await?;
    /// ```
    pub async fn send_batch_transaction(
        &mut self,
        tx_types: Vec<u8>,
        tx_infos: Vec<Value>,
    ) -> WsResult<()> {
        if tx_types.len() != tx_infos.len() {
            return Err(WsClientError::InvalidMessage(
                "tx_types and tx_infos must have same length".to_string(),
            ));
        }

        if tx_types.len() > 50 {
            return Err(WsClientError::InvalidMessage(
                "Batch transaction limit is 50 transactions".to_string(),
            ));
        }

        let token = self.auth_token.clone().ok_or_else(|| {
            WsClientError::InvalidMessage("Authentication token not set".to_string())
        })?;

        let msg_id = self.next_message_id();

        let tx_infos_as_strings: Vec<String> = tx_infos.iter().map(|v| v.to_string()).collect();
        let tx_types_json = serde_json::to_string(&tx_types)?;
        let tx_infos_json = serde_json::to_string(&tx_infos_as_strings)?;

        let msg = json!({
            "type": "jsonapi/sendtxbatch",
            "data": {
                "id": format!("batch_{}", msg_id),
                "tx_types": tx_types_json,
                "tx_infos": tx_infos_json,
                "token": token,
            }
        });

        self.stream.send(Message::Text(msg.to_string())).await?;

        Ok(())
    }

    /// Wait for transaction response with timeout
    ///
    /// # Arguments
    /// * `timeout` - Maximum time to wait for response
    ///
    /// # Returns
    /// Ok(true) if transaction succeeded, Ok(false) if failed or timeout, Err if connection error
    ///
    /// # Example
    /// ```ignore
    /// use std::time::Duration;
    /// let success = connection.wait_for_tx_response(Duration::from_secs(3)).await?;
    /// if success {
    ///     println!("Transaction succeeded");
    /// }
    /// ```
    pub async fn wait_for_tx_response(&mut self, timeout: Duration) -> WsResult<bool> {
        use tokio::time::timeout as tokio_timeout;

        let result = tokio_timeout(timeout, async {
            while let Some(message) = self.stream.next().await {
                let message = message?;
                if let Message::Text(text) = message {
                    // Skip non-transaction messages (order book updates, etc)
                    if text.contains(r#""type":"connected""#)
                        || text.contains(r#""type":"subscribed"#)  // Matches "subscribed" AND "subscribed/..."
                        || text.contains(r#""type":"unsubscribed""#)
                        || text.contains(r#""type":"pong""#)
                        || text.contains(r#""type":"update/order_book""#)
                        || text.contains(r#""type":"update/bbo""#)
                        || text.contains(r#""type":"update/trade""#)
                        || text.contains(r#""type":"update/account""#)
                        || text.contains(r#""type":"update/fills""#)
                        || text.contains(r#""type":"update/orders""#)
                    {
                        tracing::debug!("Skipping non-transaction message in wait_for_tx_response");
                        continue;
                    }

                    // Check if this is an error response
                    let is_error = text.contains(r#""type":"error""#)
                        || text.contains(r#""error":"#)
                        || (text.contains(r#""code":"#) && !text.contains(r#""code":200"#));

                    if is_error {
                        tracing::error!("Transaction error response: {}", text);
                    } else {
                        tracing::info!("Transaction success response: {}", text);
                    }

                    return Ok(!is_error);
                }
            }
            Ok(false)
        })
        .await;

        match result {
            Ok(Ok(success)) => Ok(success),
            Ok(Err(e)) => Err(e),
            Err(_) => Ok(false), // Timeout = failure
        }
    }

    /// Send a ping message to keep connection alive
    pub async fn ping(&mut self) -> WsResult<()> {
        self.stream.send(Message::Ping(vec![])).await?;
        Ok(())
    }

    /// Reconnect to WebSocket server with exponential backoff and jitter
    ///
    /// This method will attempt to reconnect and resubscribe to all channels.
    /// It uses exponential backoff with jitter to avoid thundering herd problems.
    ///
    /// # Arguments
    /// * `max_attempts` - Maximum number of reconnection attempts (None = infinite)
    ///
    /// # Returns
    /// Result indicating success or failure
    ///
    /// # Example
    /// ```ignore
    /// connection.reconnect(Some(5)).await?; // Try up to 5 times
    /// ```
    pub async fn reconnect(&mut self, max_attempts: Option<u32>) -> WsResult<()> {
        use rand::Rng;
        use tokio::time::sleep;

        let mut delay = self.backoff.initial;
        let mut attempts = 0;

        tracing::info!(generation = self.generation, "reconnect_begin");

        loop {
            if let Some(max) = max_attempts {
                if attempts >= max {
                    return Err(WsClientError::InvalidMessage(format!(
                        "Max reconnection attempts ({}) exceeded",
                        max
                    )));
                }
            }

            attempts += 1;

            // Add jitter: ±10% randomization to prevent thundering herd
            let jitter = rand::thread_rng().gen_range(0.9..1.1);
            let actual_delay = delay.mul_f64(jitter);

            tracing::warn!(
                "Reconnecting (attempt {})... waiting {:?}",
                attempts,
                actual_delay
            );

            sleep(actual_delay).await;

            let dial_start = Instant::now();
            match connect_async(self.url.as_str()).await {
                Ok((stream, _)) => {
                    let dial_elapsed = dial_start.elapsed();
                    tracing::info!(attempts, ?dial_elapsed, "reconnect_dial_success");
                    self.stream = stream;
                    self.generation = self.generation.wrapping_add(1);
                    self.suppress_already_subscribed_until =
                        Some(Instant::now() + SUPPRESS_ALREADY_SUB_DURATION);
                    self.state.order_books.clear();
                    self.state.accounts.clear();

                    let auth_start = Instant::now();
                    if let Err(e) = self.send_auth_if_needed().await {
                        tracing::error!("Failed to re-authenticate after reconnection: {}", e);
                        return Err(e);
                    }
                    let auth_elapsed = auth_start.elapsed();

                    let subs_start = Instant::now();
                    if let Err(e) = self.send_subscriptions().await {
                        tracing::error!("Failed to resubscribe after reconnection: {}", e);
                        return Err(e);
                    }
                    let subs_elapsed = subs_start.elapsed();

                    tracing::info!(
                        attempts,
                        ?dial_elapsed,
                        ?auth_elapsed,
                        ?subs_elapsed,
                        generation = self.generation,
                        "reconnect_completed"
                    );

                    return Ok(());
                }
                Err(e) => {
                    tracing::error!("Reconnection failed (attempt {}): {}", attempts, e);

                    // Exponential backoff with cap
                    delay = (delay.mul_f64(self.backoff.multiplier)).min(self.backoff.max);
                }
            }
        }
    }

    async fn handle_text_message(&mut self, text: String) -> WsResult<Option<WsEvent>> {
        let message: Value = serde_json::from_str(&text)?;

        // Gracefully handle messages without a "type" field
        let message_type = match message.get("type").and_then(|value| value.as_str()) {
            Some(msg_type) => msg_type.to_owned(),
            None => {
                // Log and return Unknown event for messages without type field
                tracing::warn!(
                    "WebSocket message missing 'type' field: {}",
                    if text.len() > 100 {
                        &text[..100]
                    } else {
                        &text
                    }
                );
                return Ok(Some(WsEvent::Unknown(text)));
            }
        };

        match message_type.as_str() {
            "connected" => {
                self.send_subscriptions().await?;
                Ok(Some(WsEvent::Connected))
            }
            "ping" => {
                // CRITICAL: Lighter uses JSON ping/pong (not WebSocket protocol)
                // Must respond within ~12 seconds or connection closes with "no pong"
                let pong_msg = json!({"type": "pong"});
                self.stream
                    .send(Message::Text(pong_msg.to_string()))
                    .await?;
                Ok(Some(WsEvent::Pong))
            }
            "subscribed/order_book" => {
                let payload: OrderBookEnvelope = serde_json::from_value(message)?;
                let market = MarketId::from(parse_market_id(&payload.channel)?);

                let envelope_offset = payload.offset;
                let payload_code = payload.code;
                let order_book_payload = payload.order_book;
                let book_code = order_book_payload.code;
                let offset = order_book_payload.offset.or(envelope_offset);

                if let Some(code) = payload_code.or(book_code) {
                    if code != 200 {
                        tracing::warn!(market = %market.0, code, "order book snapshot returned non-success code");
                    }
                }

                let snapshot = OrderBookState::from_payload(order_book_payload);
                self.state.order_books.insert(market, snapshot.clone());
                if let Some(off) = offset {
                    self.state.order_book_offsets.insert(market, off);
                } else {
                    self.state.order_book_offsets.remove(&market);
                }
                Ok(Some(WsEvent::OrderBook(OrderBookEvent {
                    market,
                    state: snapshot,
                    delta: None,
                })))
            }
            "update/order_book" => {
                let payload: OrderBookEnvelope = serde_json::from_value(message)?;
                let market = MarketId::from(parse_market_id(&payload.channel)?);

                let envelope_offset = payload.offset;
                let payload_code = payload.code;
                let order_book_payload = payload.order_book;
                let book_code = order_book_payload.code;
                let offset = order_book_payload.offset.or(envelope_offset);

                if let Some(code) = payload_code.or(book_code) {
                    if code != 200 {
                        tracing::warn!(market = %market.0, code, "order book delta returned non-success code");
                    }
                }

                if let Some(new_offset) = offset {
                    if let Some(prev) = self.state.order_book_offsets.get(&market) {
                        if new_offset != prev + 1 {
                            tracing::warn!(
                                market = %market.0,
                                expected = prev + 1,
                                actual = new_offset,
                                "order book offset gap detected"
                            );
                        }
                    }
                    self.state.order_book_offsets.insert(market, new_offset);
                }

                let delta = OrderBookDelta::from_payload(order_book_payload);

                if let Some(state) = self.state.order_books.get_mut(&market) {
                    state.apply_delta(&delta);
                    Ok(Some(WsEvent::OrderBook(OrderBookEvent {
                        market,
                        state: state.clone(),
                        delta: Some(delta),
                    })))
                } else {
                    tracing::warn!(
                        "Dropping order book delta received before snapshot for market {}",
                        market.0
                    );
                    Ok(None)
                }
            }
            "subscribed/market_stats" | "update/market_stats" => {
                let envelope: MarketStatsEnvelope = serde_json::from_value(message)?;
                Ok(Some(WsEvent::MarketStats(MarketStatsEvent {
                    channel: envelope.channel,
                    market_stats: envelope.market_stats,
                })))
            }
            "subscribed/transaction" | "update/transaction" => {
                let envelope: TransactionEnvelope = serde_json::from_value(message)?;
                Ok(Some(WsEvent::Transaction(TransactionEvent {
                    channel: envelope.channel,
                    txs: envelope.txs,
                })))
            }
            "subscribed/executed_transaction" | "update/executed_transaction" => {
                let envelope: ExecutedTransactionEnvelope = serde_json::from_value(message)?;
                Ok(Some(WsEvent::ExecutedTransaction(
                    ExecutedTransactionEvent {
                        channel: envelope.channel,
                        executed_txs: envelope.executed_txs,
                    },
                )))
            }
            "subscribed/height" | "update/height" => {
                let envelope: HeightEnvelope = serde_json::from_value(message)?;
                Ok(Some(WsEvent::Height(HeightEvent {
                    channel: envelope.channel,
                    height: envelope.height,
                })))
            }
            "subscribed/trade" | "update/trade" => {
                let envelope: TradeEnvelope = serde_json::from_value(message)?;
                Ok(Some(WsEvent::Trade(TradeEvent {
                    channel: envelope.channel,
                    trades: envelope.trades,
                })))
            }
            "subscribed/bbo" | "update/bbo" => {
                let envelope: BBOEnvelope = serde_json::from_value(message)?;
                Ok(Some(WsEvent::BBO(BBOEvent {
                    channel: envelope.channel,
                    market_id: envelope.market_id,
                    best_bid: envelope.best_bid,
                    best_ask: envelope.best_ask,
                    timestamp: envelope.timestamp,
                })))
            }
            other => {
                if let Some(snapshot) = classify_account_message(other) {
                    let event = self.handle_account_message(message, snapshot)?;
                    Ok(Some(WsEvent::Account(event)))
                } else {
                    Ok(Some(WsEvent::Unknown(text)))
                }
            }
        }
    }

    async fn send_subscriptions(&mut self) -> WsResult<()> {
        // Order books
        for market in &self.subscriptions.order_books {
            let payload = json!({
                "type": "subscribe",
                "channel": format!("order_book/{}", market.into_inner()),
            })
            .to_string();
            self.stream.send(Message::Text(payload)).await?;
        }

        // Accounts (account_all channel) - requires authentication
        for account in &self.subscriptions.accounts {
            let mut payload = json!({
                "type": "subscribe",
                "channel": format!("account_all/{}", account.into_inner()),
            });
            if let Some(token) = &self.auth_token {
                payload["auth"] = json!(token);
            }
            self.stream.send(Message::Text(payload.to_string())).await?;
        }

        // BBO (Best Bid/Offer)
        for market in &self.subscriptions.bbo {
            let payload = json!({
                "type": "subscribe",
                "channel": format!("bbo/{}", market.into_inner()),
            })
            .to_string();
            self.stream.send(Message::Text(payload)).await?;
        }

        // Trades
        for market in &self.subscriptions.trades {
            let payload = json!({
                "type": "subscribe",
                "channel": format!("trade/{}", market.into_inner()),
            })
            .to_string();
            self.stream.send(Message::Text(payload)).await?;
        }

        // Market stats
        for market in &self.subscriptions.market_stats {
            let payload = json!({
                "type": "subscribe",
                "channel": format!("market_stats/{}", market.into_inner()),
            })
            .to_string();
            self.stream.send(Message::Text(payload)).await?;
        }

        // Transactions
        if self.subscriptions.subscribe_transactions {
            let payload = json!({
                "type": "subscribe",
                "channel": "transaction",
            })
            .to_string();
            self.stream.send(Message::Text(payload)).await?;
        }

        // Executed transactions
        if self.subscriptions.subscribe_executed_transactions {
            let payload = json!({
                "type": "subscribe",
                "channel": "executed_transaction",
            })
            .to_string();
            self.stream.send(Message::Text(payload)).await?;
        }

        // Height
        if self.subscriptions.subscribe_height {
            let payload = json!({
                "type": "subscribe",
                "channel": "height",
            })
            .to_string();
            self.stream.send(Message::Text(payload)).await?;
        }

        // Account all positions - requires authentication
        for account in &self.subscriptions.account_all_positions {
            let mut payload = json!({
                "type": "subscribe",
                "channel": format!("account_all_positions/{}", account.into_inner()),
            });
            if let Some(token) = &self.auth_token {
                payload["auth"] = json!(token);
            }
            self.stream.send(Message::Text(payload.to_string())).await?;
        }

        // Account all trades - requires authentication
        for account in &self.subscriptions.account_all_trades {
            let mut payload = json!({
                "type": "subscribe",
                "channel": format!("account_all_trades/{}", account.into_inner()),
            });
            if let Some(token) = &self.auth_token {
                payload["auth"] = json!(token);
            }
            self.stream.send(Message::Text(payload.to_string())).await?;
        }

        // Account all orders - requires authentication
        for account in &self.subscriptions.account_all_orders {
            let mut payload = json!({
                "type": "subscribe",
                "channel": format!("account_all_orders/{}", account.into_inner()),
            });
            if let Some(token) = &self.auth_token {
                payload["auth"] = json!(token);
            }
            self.stream.send(Message::Text(payload.to_string())).await?;
        }

        // User stats - requires authentication
        for account in &self.subscriptions.user_stats {
            let mut payload = json!({
                "type": "subscribe",
                "channel": format!("user_stats/{}", account.into_inner()),
            });
            if let Some(token) = &self.auth_token {
                payload["auth"] = json!(token);
            }
            self.stream.send(Message::Text(payload.to_string())).await?;
        }

        // Account transactions - requires authentication
        for account in &self.subscriptions.account_tx {
            let mut payload = json!({
                "type": "subscribe",
                "channel": format!("account_tx/{}", account.into_inner()),
            });
            if let Some(token) = &self.auth_token {
                payload["auth"] = json!(token);
            }
            self.stream.send(Message::Text(payload.to_string())).await?;
        }

        // Pool data - requires authentication
        for account in &self.subscriptions.pool_data {
            let mut payload = json!({
                "type": "subscribe",
                "channel": format!("pool_data/{}", account.into_inner()),
            });
            if let Some(token) = &self.auth_token {
                payload["auth"] = json!(token);
            }
            self.stream.send(Message::Text(payload.to_string())).await?;
        }

        // Pool info - requires authentication
        for account in &self.subscriptions.pool_info {
            let mut payload = json!({
                "type": "subscribe",
                "channel": format!("pool_info/{}", account.into_inner()),
            });
            if let Some(token) = &self.auth_token {
                payload["auth"] = json!(token);
            }
            self.stream.send(Message::Text(payload.to_string())).await?;
        }

        // Notifications - requires authentication
        for account in &self.subscriptions.notifications {
            let mut payload = json!({
                "type": "subscribe",
                "channel": format!("notification/{}", account.into_inner()),
            });
            if let Some(token) = &self.auth_token {
                payload["auth"] = json!(token);
            }
            self.stream.send(Message::Text(payload.to_string())).await?;
        }

        // Account market combined channel - requires authentication
        for (market, account) in &self.subscriptions.account_market {
            let mut payload = json!({
                "type": "subscribe",
                "channel": format!("account_market/{}/{}", market.into_inner(), account.into_inner()),
            });
            if let Some(token) = &self.auth_token {
                payload["auth"] = json!(token);
            }
            self.stream.send(Message::Text(payload.to_string())).await?;
        }

        // Market-specific account orders - requires authentication
        for (market, account) in &self.subscriptions.account_market_orders {
            let mut payload = json!({
                "type": "subscribe",
                "channel": format!("account_orders/{}/{}", market.into_inner(), account.into_inner()),
            });
            if let Some(token) = &self.auth_token {
                payload["auth"] = json!(token);
            }
            self.stream.send(Message::Text(payload.to_string())).await?;
        }

        // Market-specific account positions - requires authentication
        for (market, account) in &self.subscriptions.account_market_positions {
            let mut payload = json!({
                "type": "subscribe",
                "channel": format!("account_positions/{}/{}", market.into_inner(), account.into_inner()),
            });
            if let Some(token) = &self.auth_token {
                payload["auth"] = json!(token);
            }
            self.stream.send(Message::Text(payload.to_string())).await?;
        }

        // Market-specific account trades - requires authentication
        for (market, account) in &self.subscriptions.account_market_trades {
            let mut payload = json!({
                "type": "subscribe",
                "channel": format!("account_trades/{}/{}", market.into_inner(), account.into_inner()),
            });
            if let Some(token) = &self.auth_token {
                payload["auth"] = json!(token);
            }
            self.stream.send(Message::Text(payload.to_string())).await?;
        }

        Ok(())
    }

    fn handle_account_message(
        &mut self,
        mut message: Value,
        snapshot: bool,
    ) -> WsResult<AccountEventEnvelope> {
        let channel = message
            .get("channel")
            .and_then(|value| value.as_str())
            .ok_or_else(|| WsClientError::InvalidChannel("missing channel".to_string()))?;
        let account = AccountId::from(parse_account_index(channel)?);

        if let Some(obj) = message.as_object_mut() {
            obj.remove("type");
        }

        let event = AccountEvent::new(message.clone());
        self.state.accounts.insert(account, event.clone());

        Ok(AccountEventEnvelope {
            account,
            snapshot,
            event,
        })
    }

}

pub struct WsStream {
    connection: WsConnection,
}

impl WsStream {
    fn new(connection: WsConnection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &WsConnection {
        &self.connection
    }

    pub fn connection_mut(&mut self) -> &mut WsConnection {
        &mut self.connection
    }

    pub fn into_connection(self) -> WsConnection {
        self.connection
    }
}

impl Stream for WsStream {
    type Item = WsResult<WsEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let fut = self.connection.next_event();
        futures_util::pin_mut!(fut);
        match futures_util::ready!(fut.poll_unpin(cx)) {
            Ok(Some(event)) => Poll::Ready(Some(Ok(event))),
            Ok(None) => Poll::Ready(None),
            Err(err) => Poll::Ready(Some(Err(err))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CloseFrameInfo {
    pub code: u16,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub enum WsEvent {
    Connected,
    Pong,
    OrderBook(OrderBookEvent),
    Account(AccountEventEnvelope),
    MarketStats(MarketStatsEvent),
    Transaction(TransactionEvent),
    ExecutedTransaction(ExecutedTransactionEvent),
    Height(HeightEvent),
    Trade(TradeEvent),
    BBO(BBOEvent),
    Closed(Option<CloseFrameInfo>),
    Unknown(String),
}

#[derive(Debug, Clone)]
pub struct OrderBookEvent {
    pub market: MarketId,
    pub state: OrderBookState,
    pub delta: Option<OrderBookDelta>,
}

#[derive(Debug, Clone)]
pub struct AccountEventEnvelope {
    pub account: AccountId,
    pub snapshot: bool,
    pub event: AccountEvent,
}

#[derive(Debug, Clone)]
pub struct AccountEvent(Value);

impl AccountEvent {
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    pub fn into_inner(self) -> Value {
        self.0
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OrderBookEnvelope {
    channel: String,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    code: Option<i32>,
    #[serde(rename = "order_book")]
    order_book: OrderBookPayload,
}

#[derive(Debug, Clone, Deserialize)]
struct OrderBookPayload {
    asks: Vec<OrderBookLevel>,
    bids: Vec<OrderBookLevel>,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OrderBookLevel {
    pub price: String,
    #[serde(default)]
    pub size: String,
    #[serde(rename = "remaining_base_amount", default)]
    pub remaining_base_amount: Option<String>,
    #[serde(flatten, default)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct OrderBookDelta {
    pub asks: Vec<OrderBookLevel>,
    pub bids: Vec<OrderBookLevel>,
}

impl OrderBookDelta {
    fn from_payload(payload: OrderBookPayload) -> Self {
        Self {
            asks: payload.asks,
            bids: payload.bids,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OrderBookState {
    pub asks: Vec<OrderBookLevel>,
    pub bids: Vec<OrderBookLevel>,
}

impl OrderBookState {
    fn from_payload(payload: OrderBookPayload) -> Self {
        let mut asks = payload.asks;
        let mut bids = payload.bids;
        sort_levels(&mut asks, false);
        sort_levels(&mut bids, true);
        Self { asks, bids }
    }

    fn from_delta(delta: &OrderBookDelta) -> Self {
        let mut asks = delta.asks.clone();
        let mut bids = delta.bids.clone();
        sort_levels(&mut asks, false);
        sort_levels(&mut bids, true);
        Self { asks, bids }
    }

    fn apply_delta(&mut self, delta: &OrderBookDelta) {
        update_side(&mut self.asks, &delta.asks, false);
        update_side(&mut self.bids, &delta.bids, true);
    }
}

// Market Stats Event Structures
#[derive(Debug, Clone)]
pub struct MarketStatsEvent {
    pub channel: String,
    pub market_stats: MarketStats,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MarketStats {
    pub market_id: u32,
    pub index_price: String,
    pub mark_price: String,
    pub open_interest: String,
    pub open_interest_limit: String,
    pub funding_clamp_small: String,
    pub funding_clamp_big: String,
    pub last_trade_price: String,
    pub current_funding_rate: String,
    pub funding_rate: String,
    pub funding_timestamp: i64,
    pub daily_base_token_volume: f64,
    pub daily_quote_token_volume: f64,
    pub daily_price_low: f64,
    pub daily_price_high: f64,
    pub daily_price_change: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct MarketStatsEnvelope {
    channel: String,
    market_stats: MarketStats,
}

// Transaction Event Structures
#[derive(Debug, Clone)]
pub struct TransactionEvent {
    pub channel: String,
    pub txs: Vec<TransactionData>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransactionData {
    pub hash: String,
    #[serde(rename = "type")]
    pub tx_type: u32,
    pub info: String,
    pub event_info: String,
    pub status: u32,
    pub transaction_index: i64,
    pub l1_address: String,
    pub account_index: u64,
    pub nonce: u64,
    pub expire_at: i64,
    pub block_height: i64,
    pub queued_at: i64,
    pub executed_at: i64,
    pub sequence_index: i64,
    pub parent_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TransactionEnvelope {
    channel: String,
    #[serde(default)]
    txs: Vec<TransactionData>,
}

// Executed Transaction Event Structure
#[derive(Debug, Clone)]
pub struct ExecutedTransactionEvent {
    pub channel: String,
    pub executed_txs: Vec<TransactionData>,
}

#[derive(Debug, Clone, Deserialize)]
struct ExecutedTransactionEnvelope {
    channel: String,
    #[serde(default)]
    executed_txs: Vec<TransactionData>,
}

// Height Event Structures
#[derive(Debug, Clone)]
pub struct HeightEvent {
    pub channel: String,
    pub height: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct HeightEnvelope {
    channel: String,
    height: i64,
}

// Trade Event Structures
#[derive(Debug, Clone)]
pub struct TradeEvent {
    pub channel: String,
    pub trades: Vec<TradeData>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TradeData {
    #[serde(default)] // Optional during subscription confirmation
    pub market_id: u32,
    #[serde(default)] // Optional during subscription confirmation
    pub side: String,
    #[serde(default)] // Optional during subscription confirmation
    pub price: String,
    #[serde(default)] // Optional during subscription confirmation
    pub base_size: String,
    #[serde(default)] // Optional during subscription confirmation
    pub quote_size: String,
    #[serde(default)] // Optional during subscription confirmation
    pub timestamp: i64,
    #[serde(default)]
    pub is_liquidation: bool,
    #[serde(default)]
    pub liquidation: Option<LiquidationData>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LiquidationData {
    pub account: String,
    pub is_maker: bool,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct TradeEnvelope {
    channel: String,
    #[serde(default)]
    trades: Vec<TradeData>,
}

// BBO (Best Bid/Offer) Event Structures
#[derive(Debug, Clone)]
pub struct BBOEvent {
    pub channel: String,
    pub market_id: u32,
    pub best_bid: Option<String>,
    pub best_ask: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct BBOEnvelope {
    channel: String,
    market_id: u32,
    best_bid: Option<String>,
    best_ask: Option<String>,
    timestamp: i64,
}

fn update_side(levels: &mut Vec<OrderBookLevel>, updates: &[OrderBookLevel], descending: bool) {
    for update in updates {
        let update_key = normalize_price_str(&update.price);
        let size_zero = parse_f64(&update.size).map(|v| v == 0.0).unwrap_or(false);
        let remaining_zero = update
            .remaining_base_amount
            .as_deref()
            .and_then(parse_f64)
            .map(|v| v == 0.0)
            .unwrap_or(false);

        if size_zero || remaining_zero {
            if let Some(pos) = levels
                .iter()
                .position(|level| normalize_price_str(&level.price) == update_key)
            {
                levels.remove(pos);
            }
            continue;
        }

        if let Some(existing) = levels
            .iter_mut()
            .find(|level| normalize_price_str(&level.price) == update_key)
        {
            *existing = update.clone();
        } else {
            levels.push(update.clone());
        }
    }
    levels.retain(|level| !level_is_zero(level));
    sort_levels(levels, descending);
}

fn parse_f64(text: &str) -> Option<f64> {
    text.parse::<f64>().ok()
}

fn normalize_price_str(text: &str) -> String {
    match parse_f64(text) {
        Some(value) => format!("{value:.8}"),
        None => text.to_string(),
    }
}

fn level_is_zero(level: &OrderBookLevel) -> bool {
    let size_zero = parse_f64(&level.size)
        .map(|value| value == 0.0)
        .unwrap_or(false);
    let remaining_zero = level
        .remaining_base_amount
        .as_deref()
        .and_then(parse_f64)
        .map(|value| value == 0.0)
        .unwrap_or(false);
    size_zero || remaining_zero
}

fn sort_levels(levels: &mut Vec<OrderBookLevel>, descending: bool) {
    use std::cmp::Ordering;

    levels.sort_by(|a, b| {
        let price_a = parse_f64(&a.price).unwrap_or(0.0);
        let price_b = parse_f64(&b.price).unwrap_or(0.0);
        match price_a.partial_cmp(&price_b) {
            Some(ordering) => {
                if descending {
                    ordering.reverse()
                } else {
                    ordering
                }
            }
            None => Ordering::Equal,
        }
    });
}

fn build_url(config: &WsConfig) -> WsResult<Url> {
    let mut candidate = config.host.clone();
    if candidate.starts_with("https://") {
        candidate = candidate.replacen("https://", "wss://", 1);
    } else if candidate.starts_with("http://") {
        candidate = candidate.replacen("http://", "ws://", 1);
    } else if !candidate.starts_with("ws://") && !candidate.starts_with("wss://") {
        candidate = format!("wss://{candidate}");
    }

    let mut url = Url::parse(&candidate)?;
    url.set_path(&config.path);
    Ok(url)
}

fn parse_market_id(channel: &str) -> WsResult<i32> {
    channel
        .split(|c| c == '/' || c == ':')
        .last()
        .and_then(|value| value.parse::<i32>().ok())
        .ok_or_else(|| WsClientError::InvalidChannel(channel.to_string()))
}

fn parse_account_index(channel: &str) -> WsResult<i64> {
    channel
        .split(|c| c == '/' || c == ':')
        .last()
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| WsClientError::InvalidChannel(channel.to_string()))
}

fn classify_account_message(message_type: &str) -> Option<bool> {
    const ACCOUNT_CHANNELS: &[&str] = &[
        "account",
        "account_all",
        "account_all_orders",
        "account_all_trades",
        "account_all_positions",
        "account_market",
        "account_orders",
        "account_positions",
        "account_trades",
        "account_tx",
        "user_stats",
        "pool_data",
        "pool_info",
        "notification",
    ];

    if let Some(rest) = message_type.strip_prefix("subscribed/") {
        if ACCOUNT_CHANNELS.contains(&rest) {
            return Some(true);
        }
    }
    if let Some(rest) = message_type.strip_prefix("update/") {
        if ACCOUNT_CHANNELS.contains(&rest) {
            return Some(false);
        }
    }
    None
}
