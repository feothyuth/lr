use super::types::{QuoteOrder, QuotePair, StrategyDecision};
use crate::{
    lighter_client::LighterClient,
    signer_client::BatchEntry,
    tx_executor::{
        send_batch_tx_ws_with_mode, BatchAckMode, TX_TYPE_CANCEL_ORDER, TX_TYPE_CREATE_ORDER,
    },
    types::{ApiKeyIndex, BaseQty, MarketId, Nonce, Price},
    ws_client::{AccountEvent, TransactionData, WsConnection},
};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{mpsc, oneshot, Mutex},
    time::sleep,
};
use tracing::{debug, error, info, warn};

const DECISION_CHANNEL_DEPTH: usize = 64;
const DIRECTIVE_CHANNEL_DEPTH: usize = 64;
const SIGNED_CHANNEL_DEPTH: usize = 64;
const ACCOUNT_CHANNEL_DEPTH: usize = 128;
const TX_ACK_CHANNEL_DEPTH: usize = 64;
const CONTROL_CHANNEL_DEPTH: usize = 8;

const INITIAL_BACKOFF_MS: u64 = 50;
const MAX_BACKOFF_MS: u64 = 1_000;
const MAX_RETRY_ATTEMPTS: u8 = 5;
const FAST_MODE_TIMEOUT_MS: u64 = 100;
const MAX_SAFE_LIVE_ORDERS: usize = 10;
const EMERGENCY_COOLDOWN_MIN: Duration = Duration::from_secs(5);
const EMERGENCY_COOLDOWN_MAX: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum OrderSide {
    Bid,
    Ask,
}

impl OrderSide {
    fn as_str(self) -> &'static str {
        match self {
            Self::Bid => "bid",
            Self::Ask => "ask",
        }
    }

    fn is_ask(self) -> bool {
        matches!(self, Self::Ask)
    }

    fn from_is_ask(value: bool) -> Self {
        if value {
            Self::Ask
        } else {
            Self::Bid
        }
    }
}

#[derive(Clone, Debug)]
enum PendingAction {
    Create { price_ticks: i64 },
    Cancel { order_index: u64 },
}

impl PendingAction {
    fn price_ticks(&self) -> Option<i64> {
        match self {
            PendingAction::Create { price_ticks } => Some(*price_ticks),
            PendingAction::Cancel { .. } => None,
        }
    }
}

#[derive(Clone, Debug)]
struct PendingOrder {
    client_order_id: i64,
    side: OrderSide,
    _version: u64,
    action: PendingAction,
    _created_at: Instant,
    submitted_at: Option<Instant>,
    last_attempt: Option<Instant>,
    attempts: u8,
    retry_budget: u8,
    backoff_ms: u64,
    nonce: Option<i64>,
    api_key_index: Option<i32>,
    superseded: bool,
}

impl PendingOrder {
    fn new(client_order_id: i64, side: OrderSide, version: u64, action: PendingAction) -> Self {
        Self {
            client_order_id,
            side,
            _version: version,
            action,
            _created_at: Instant::now(),
            submitted_at: None,
            last_attempt: None,
            attempts: 0,
            retry_budget: MAX_RETRY_ATTEMPTS,
            backoff_ms: INITIAL_BACKOFF_MS,
            nonce: None,
            api_key_index: None,
            superseded: false,
        }
    }

    fn mark_superseded(&mut self) {
        self.superseded = true;
    }

    fn record_attempt(&mut self) {
        self.attempts = self.attempts.saturating_add(1);
        self.last_attempt = Some(Instant::now());
        self.backoff_ms = (self.backoff_ms * 2).min(MAX_BACKOFF_MS);
        if self.retry_budget > 0 {
            self.retry_budget -= 1;
        }
    }

    fn mark_submitted(&mut self) {
        self.submitted_at = Some(Instant::now());
    }

    fn set_nonce(&mut self, nonce: Option<i64>) {
        self.nonce = nonce;
    }

    fn set_api_key(&mut self, api_key: Option<i32>) {
        self.api_key_index = api_key;
    }

    fn exhausted(&self) -> bool {
        self.retry_budget == 0
    }
}

#[derive(Clone, Debug)]
struct LiveOrder {
    order_index: u64,
    client_order_id: i64,
    side: OrderSide,
    price_ticks: i64,
}

#[derive(Debug, Default)]
struct SideSlot {
    latest_version: u64,
    desired_ticks: Option<i64>,
    desired_updated_at: Option<Instant>,
    live_order_index: Option<u64>,
    pending_creates: Vec<i64>,
    pending_cancels: Vec<i64>,
    last_submission: Option<Instant>,
}

impl SideSlot {
    fn bump_version(&mut self) -> u64 {
        self.latest_version = self.latest_version.saturating_add(1);
        self.latest_version
    }

    fn register_pending_create(&mut self, client_id: i64) {
        if !self.pending_creates.contains(&client_id) {
            self.pending_creates.push(client_id);
        }
    }

    fn register_pending_cancel(&mut self, client_id: i64) {
        if !self.pending_cancels.contains(&client_id) {
            self.pending_cancels.push(client_id);
        }
    }

    fn clear_pending_create(&mut self, client_id: i64) {
        self.pending_creates.retain(|id| *id != client_id);
    }

    fn clear_pending_cancel(&mut self, client_id: i64) {
        self.pending_cancels.retain(|id| *id != client_id);
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct PendingNonce {
    client_id: i64,
    api_key_index: Option<i32>,
}

struct ExecutionState {
    market: MarketId,
    base_qty: BaseQty,
    tick_size: f64,
    refresh_interval: Duration,
    refresh_tolerance_ticks: i64,
    dry_run: bool,
    fast_execution: bool,
    optimistic_acks: bool,
    pending_by_client: HashMap<i64, PendingOrder>,
    pending_by_nonce: HashMap<i64, PendingNonce>,
    live_by_order: HashMap<u64, LiveOrder>,
    live_by_client: HashMap<i64, u64>,
    per_side: HashMap<OrderSide, SideSlot>,
    directive_seq: u64,
    client_id_high_water: i64,
    emergency_last_triggered: Option<Instant>,
    emergency_cooldown: Duration,
}

impl ExecutionState {
    fn new(
        market: MarketId,
        base_qty: BaseQty,
        tick_size: f64,
        refresh_interval_ms: u64,
        refresh_tolerance_ticks: i64,
        dry_run: bool,
        fast_execution: bool,
        optimistic_acks: bool,
    ) -> Self {
        let mut per_side = HashMap::new();
        per_side.insert(OrderSide::Bid, SideSlot::default());
        per_side.insert(OrderSide::Ask, SideSlot::default());

        Self {
            market,
            base_qty,
            tick_size,
            refresh_interval: Duration::from_millis(refresh_interval_ms.max(10)),
            refresh_tolerance_ticks: refresh_tolerance_ticks.max(0),
            dry_run,
            fast_execution,
            optimistic_acks,
            pending_by_client: HashMap::new(),
            pending_by_nonce: HashMap::new(),
            live_by_order: HashMap::new(),
            live_by_client: HashMap::new(),
            per_side,
            directive_seq: 0,
            client_id_high_water: 0,
            emergency_last_triggered: None,
            emergency_cooldown: EMERGENCY_COOLDOWN_MIN,
        }
    }

    fn next_directive_id(&mut self) -> u64 {
        self.directive_seq = self.directive_seq.wrapping_add(1);
        self.directive_seq
    }

    fn emergency_cancel_required(&self) -> bool {
        self.live_by_order.len() > MAX_SAFE_LIVE_ORDERS
    }

    fn to_ticks(&self, price: f64, side: OrderSide) -> i64 {
        if price <= 0.0 || self.tick_size <= 0.0 {
            return 1;
        }
        let ticks = price / self.tick_size;
        let rounded = if side.is_ask() {
            ticks.ceil()
        } else {
            ticks.floor()
        };
        rounded.max(1.0) as i64
    }

    fn log_live_order_summary(&self) {
        if self.live_by_order.is_empty() {
            return;
        }
        let bid_count = self
            .live_by_order
            .values()
            .filter(|o| matches!(o.side, OrderSide::Bid))
            .count();
        let ask_count = self
            .live_by_order
            .values()
            .filter(|o| matches!(o.side, OrderSide::Ask))
            .count();
        debug!(
            "Live orders: {} total (bid: {}, ask: {})",
            self.live_by_order.len(),
            bid_count,
            ask_count
        );
    }

    fn next_client_order_id(&mut self) -> i64 {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or_else(|_| Instant::now().elapsed().as_millis() as i64);
        let candidate = now_ms << 4;
        if candidate <= self.client_id_high_water {
            self.client_id_high_water += 1;
        } else {
            self.client_id_high_water = candidate;
        }
        self.client_id_high_water
    }

    fn side_slot_mut(&mut self, side: OrderSide) -> &mut SideSlot {
        self.per_side.get_mut(&side).expect("side slot must exist")
    }

    fn register_pending(
        &mut self,
        side: OrderSide,
        version: u64,
        action: PendingAction,
        client_id_override: Option<i64>,
    ) -> i64 {
        let client_id = client_id_override.unwrap_or_else(|| self.next_client_order_id());
        let order = PendingOrder::new(client_id, side, version, action.clone());

        match &action {
            PendingAction::Create { .. } => {
                self.side_slot_mut(side).register_pending_create(client_id);
            }
            PendingAction::Cancel { .. } => {
                self.side_slot_mut(side).register_pending_cancel(client_id);
            }
        }

        if let Entry::Occupied(mut entry) = self.pending_by_client.entry(client_id) {
            entry.get_mut().mark_superseded();
            entry.insert(order);
        } else {
            self.pending_by_client.insert(client_id, order);
        }
        client_id
    }

    fn track_signed_signature(
        &mut self,
        client_id: i64,
        nonce: Option<i64>,
        api_key_index: Option<i32>,
    ) {
        if let Some(order) = self.pending_by_client.get_mut(&client_id) {
            order.mark_submitted();
            order.set_nonce(nonce);
            order.set_api_key(api_key_index);
            if let Some(value) = nonce {
                self.pending_by_nonce.insert(
                    value,
                    PendingNonce {
                        client_id,
                        api_key_index,
                    },
                );
            }
        }
    }

    fn record_submission_failure(&mut self, client_id: i64) -> Option<PendingOrder> {
        if let Some(mut order) = self.pending_by_client.remove(&client_id) {
            if let Some(nonce) = order.nonce {
                self.pending_by_nonce.remove(&nonce);
            }
            order.record_attempt();
            if order.exhausted() {
                match order.action {
                    PendingAction::Create { .. } => {
                        self.side_slot_mut(order.side)
                            .clear_pending_create(client_id);
                    }
                    PendingAction::Cancel { .. } => {
                        self.side_slot_mut(order.side)
                            .clear_pending_cancel(client_id);
                    }
                }
                warn!(
                    "Dropping pending order {} on {} after exhausting retries",
                    client_id,
                    order.side.as_str()
                );
                None
            } else {
                match order.action {
                    PendingAction::Create { .. } => {
                        self.side_slot_mut(order.side)
                            .register_pending_create(client_id);
                    }
                    PendingAction::Cancel { .. } => {
                        self.side_slot_mut(order.side)
                            .register_pending_cancel(client_id);
                    }
                }
                Some(order)
            }
        } else {
            None
        }
    }

    fn mark_live(&mut self, live: LiveOrder) {
        self.side_slot_mut(live.side).live_order_index = Some(live.order_index);
        self.live_by_client
            .insert(live.client_order_id, live.order_index);
        self.live_by_order.insert(live.order_index, live);
    }

    fn remove_live_by_client(&mut self, client_id: i64) -> Option<LiveOrder> {
        if let Some(order_index) = self.live_by_client.remove(&client_id) {
            self.side_slot_mut(
                self.live_by_order
                    .get(&order_index)
                    .map(|order| order.side)
                    .unwrap_or(OrderSide::Bid),
            )
            .live_order_index
            .take();
            self.live_by_order.remove(&order_index)
        } else {
            None
        }
    }

    fn remove_live_by_index(&mut self, order_index: u64) -> Option<LiveOrder> {
        if let Some(live) = self.live_by_order.remove(&order_index) {
            self.side_slot_mut(live.side).live_order_index = None;
            self.live_by_client.remove(&live.client_order_id);
            Some(live)
        } else {
            None
        }
    }

    #[allow(dead_code)]
    fn desired_ticks(&self, side: OrderSide) -> Option<i64> {
        self.per_side.get(&side).and_then(|slot| slot.desired_ticks)
    }

    fn record_submission(&mut self, side: OrderSide, now: Instant) {
        self.side_slot_mut(side).last_submission = Some(now);
    }

    fn last_submission_elapsed(&self, side: OrderSide, now: Instant) -> Option<Duration> {
        self.per_side
            .get(&side)
            .and_then(|slot| slot.last_submission)
            .map(|last| now.saturating_duration_since(last))
    }

    fn compute_refresh_guard(&self, side: OrderSide, now: Instant) -> bool {
        match self.last_submission_elapsed(side, now) {
            Some(elapsed) => elapsed >= self.refresh_interval,
            None => true,
        }
    }
}

#[derive(Clone, Debug)]
struct OrderPlacement {
    side: OrderSide,
    price_ticks: i64,
    client_order_id: i64,
}

#[derive(Clone, Debug)]
struct OrderCancel {
    side: OrderSide,
    order_index: u64,
    client_order_id: i64,
}

#[derive(Clone, Debug)]
enum OrderOperation {
    Place(OrderPlacement),
    Cancel(OrderCancel),
}

impl OrderOperation {
    fn side(&self) -> OrderSide {
        match self {
            OrderOperation::Place(op) => op.side,
            OrderOperation::Cancel(op) => op.side,
        }
    }
}

#[derive(Clone, Debug)]
struct OrderDirective {
    id: u64,
    attempt: u8,
    operations: Vec<OrderOperation>,
}

impl OrderDirective {
    fn new(id: u64, operations: Vec<OrderOperation>) -> Self {
        Self {
            id,
            attempt: 0,
            operations,
        }
    }

    fn with_attempt(mut self, attempt: u8) -> Self {
        self.attempt = attempt;
        self
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct SignedOperation {
    operation: OrderOperation,
    batch_entry: BatchEntry,
    tx_type: u8,
    api_key_index: Option<i32>,
}

impl SignedOperation {
    fn new(
        operation: OrderOperation,
        batch_entry: BatchEntry,
        tx_type: u8,
        api_key_index: Option<i32>,
    ) -> Self {
        Self {
            operation,
            batch_entry,
            tx_type,
            api_key_index,
        }
    }
}

#[derive(Clone, Debug)]
struct SignedDirective {
    id: u64,
    attempt: u8,
    operations: Vec<SignedOperation>,
}

impl SignedDirective {
    fn new(id: u64, attempt: u8, operations: Vec<SignedOperation>) -> Self {
        Self {
            id,
            attempt,
            operations,
        }
    }
}

struct DecisionCommand {
    decision: StrategyDecision,
    timestamp: Instant,
}

struct AccountOrdersUpdate {
    _snapshot: bool,
    payload: Value,
}

enum ControlCommand {
    Reconnect(oneshot::Sender<Result<bool>>),
}

pub struct ExecutionEngine {
    decision_tx: mpsc::Sender<DecisionCommand>,
    account_tx: mpsc::Sender<AccountOrdersUpdate>,
    tx_ack_tx: mpsc::Sender<Vec<TransactionData>>,
    control_tx: mpsc::Sender<ControlCommand>,
}

impl ExecutionEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: Arc<LighterClient>,
        market: MarketId,
        base_qty: BaseQty,
        tick_size: f64,
        dry_run: bool,
        refresh_interval_ms: u64,
        fast_execution: bool,
        optimistic_acks: bool,
        tx_connection: WsConnection,
        refresh_tolerance_ticks: i64,
        auth_token: Option<String>,
    ) -> Self {
        let state = Arc::new(Mutex::new(ExecutionState::new(
            market,
            base_qty,
            tick_size,
            refresh_interval_ms,
            refresh_tolerance_ticks,
            dry_run,
            fast_execution,
            optimistic_acks,
        )));
        let connection = Arc::new(Mutex::new(tx_connection));
        let auth_state = Arc::new(Mutex::new(auth_token));

        let (decision_tx, decision_rx) = mpsc::channel(DECISION_CHANNEL_DEPTH);
        let (directive_tx, directive_rx) = mpsc::channel(DIRECTIVE_CHANNEL_DEPTH);
        let (signed_tx, signed_rx) = mpsc::channel(SIGNED_CHANNEL_DEPTH);
        let (account_tx, account_rx) = mpsc::channel(ACCOUNT_CHANNEL_DEPTH);
        let (tx_ack_tx, tx_ack_rx) = mpsc::channel(TX_ACK_CHANNEL_DEPTH);
        let (control_tx, control_rx) = mpsc::channel(CONTROL_CHANNEL_DEPTH);

        spawn_decision_loop(decision_rx, directive_tx.clone(), Arc::clone(&state));
        spawn_signing_loop(
            directive_rx,
            signed_tx,
            Arc::clone(&state),
            Arc::clone(&client),
        );
        spawn_submission_loop(
            signed_rx,
            directive_tx.clone(),
            Arc::clone(&state),
            Arc::clone(&connection),
        );
        spawn_account_reconciliation_loop(account_rx, directive_tx.clone(), Arc::clone(&state));
        spawn_ack_loop(tx_ack_rx, directive_tx.clone(), Arc::clone(&state));
        spawn_control_loop(
            control_rx,
            Arc::clone(&connection),
            Arc::clone(&client),
            Arc::clone(&state),
            Arc::clone(&auth_state),
        );

        Self {
            decision_tx,
            account_tx,
            tx_ack_tx,
            control_tx,
        }
    }

    pub async fn handle_decision(
        &self,
        decision: StrategyDecision,
        timestamp: Instant,
    ) -> Result<bool> {
        self.decision_tx
            .send(DecisionCommand {
                decision,
                timestamp,
            })
            .await
            .map_err(|_| anyhow!("execution pipeline shut down"))?;
        Ok(true)
    }

    pub async fn ingest_account_event(&self, snapshot: bool, event: &AccountEvent) -> Result<()> {
        let payload = event.as_value().clone();
        self.account_tx
            .send(AccountOrdersUpdate {
                _snapshot: snapshot,
                payload,
            })
            .await
            .map_err(|_| anyhow!("execution pipeline offline"))?;
        Ok(())
    }

    pub async fn ingest_transaction_event(&self, txs: Vec<TransactionData>) -> Result<()> {
        if txs.is_empty() {
            return Ok(());
        }
        self.tx_ack_tx
            .send(txs)
            .await
            .map_err(|_| anyhow!("execution pipeline offline"))?;
        Ok(())
    }

    pub async fn reconnect_transactions(&self) -> Result<bool> {
        let (tx, rx) = oneshot::channel();
        self.control_tx
            .send(ControlCommand::Reconnect(tx))
            .await
            .map_err(|_| anyhow!("execution control channel offline"))?;
        rx.await.map_err(|_| anyhow!("reconnect handler dropped"))?
    }
}

#[derive(Debug, Clone, Deserialize)]
struct AccountOrderSnapshot {
    #[serde(rename = "order_index")]
    order_index: Option<u64>,
    #[serde(rename = "client_order_index")]
    client_order_index: Option<i64>,
    #[serde(rename = "market_index")]
    market_index: Option<u32>,
    #[serde(rename = "price")]
    price: Option<String>,
    #[serde(rename = "is_ask")]
    is_ask: Option<bool>,
    #[serde(rename = "status")]
    status: Option<String>,
}

fn spawn_decision_loop(
    mut rx: mpsc::Receiver<DecisionCommand>,
    directive_tx: mpsc::Sender<OrderDirective>,
    state: Arc<Mutex<ExecutionState>>,
) {
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            if let Err(err) = handle_decision(
                state.clone(),
                directive_tx.clone(),
                cmd.decision,
                cmd.timestamp,
            )
            .await
            {
                warn!("Decision handling failed: {err:#}");
            }
        }
    });
}

async fn handle_decision(
    state: Arc<Mutex<ExecutionState>>,
    directive_tx: mpsc::Sender<OrderDirective>,
    decision: StrategyDecision,
    timestamp: Instant,
) -> Result<()> {
    {
        let mut guard = state.lock().await;
        if guard.emergency_cancel_required() {
            let should_trigger = guard
                .emergency_last_triggered
                .map(|last| last.elapsed() >= guard.emergency_cooldown)
                .unwrap_or(true);
            if !should_trigger {
                let remaining = guard
                    .emergency_last_triggered
                    .map(|last| guard.emergency_cooldown.saturating_sub(last.elapsed()))
                    .unwrap_or(Duration::ZERO);
                debug!("Emergency breaker cooling down ({:?} remaining)", remaining);
                drop(guard);
                return Ok(());
            }

            guard.emergency_last_triggered = Some(Instant::now());
            guard.emergency_cooldown = (guard.emergency_cooldown * 2).min(EMERGENCY_COOLDOWN_MAX);
            let live_count = guard.live_by_order.len();
            let current_cooldown = guard.emergency_cooldown;
            drop(guard);

            let directives = {
                let mut guard = state.lock().await;
                plan_cancel_all(&mut *guard, "emergency_live_order_guard", timestamp)
            };
            for directive in directives {
                directive_tx
                    .send(directive)
                    .await
                    .map_err(|_| anyhow!("directive channel closed"))?;
            }
            warn!(
                "Emergency cancel-all triggered due to {} live orders (cooldown: {:?})",
                live_count, current_cooldown
            );
            return Ok(());
        } else if guard.emergency_last_triggered.is_some() {
            guard.emergency_last_triggered = None;
            guard.emergency_cooldown = EMERGENCY_COOLDOWN_MIN;
            info!("Emergency breaker reset - order count back to healthy levels");
        }
    }

    match decision {
        StrategyDecision::Skip(reason) => {
            debug!("Execution skip: {reason}");
            Ok(())
        }
        StrategyDecision::Cancel(reason) => {
            let directives = {
                let mut guard = state.lock().await;
                plan_cancel_all(&mut *guard, reason, timestamp)
            };
            for directive in directives {
                directive_tx
                    .send(directive)
                    .await
                    .map_err(|_| anyhow!("directive channel closed"))?;
            }
            Ok(())
        }
        StrategyDecision::Quote(quotes) => {
            schedule_quotes(state, directive_tx, quotes, timestamp).await
        }
        StrategyDecision::QuoteBidOnly(bid) => {
            schedule_one_side(state, directive_tx, OrderSide::Bid, Some(bid), timestamp).await
        }
        StrategyDecision::QuoteAskOnly(ask) => {
            schedule_one_side(state, directive_tx, OrderSide::Ask, Some(ask), timestamp).await
        }
    }
}

async fn schedule_quotes(
    state: Arc<Mutex<ExecutionState>>,
    directive_tx: mpsc::Sender<OrderDirective>,
    quotes: QuotePair,
    timestamp: Instant,
) -> Result<()> {
    let (bid_ops, ask_ops, directive_id) = {
        let mut guard = state.lock().await;
        let bid_ops = plan_side(&mut *guard, OrderSide::Bid, Some(quotes.bid), timestamp)?;
        let ask_ops = plan_side(&mut *guard, OrderSide::Ask, Some(quotes.ask), timestamp)?;
        let id = if !bid_ops.is_empty() || !ask_ops.is_empty() {
            Some(guard.next_directive_id())
        } else {
            None
        };
        (bid_ops, ask_ops, id)
    };

    if let Some(id) = directive_id {
        let mut operations = Vec::new();
        operations.extend(bid_ops);
        operations.extend(ask_ops);
        if !operations.is_empty() {
            directive_tx
                .send(OrderDirective::new(id, operations))
                .await
                .map_err(|_| anyhow!("directive channel closed"))?;
        }
    }
    Ok(())
}

async fn schedule_one_side(
    state: Arc<Mutex<ExecutionState>>,
    directive_tx: mpsc::Sender<OrderDirective>,
    side: OrderSide,
    order: Option<QuoteOrder>,
    timestamp: Instant,
) -> Result<()> {
    let operations = {
        let mut guard = state.lock().await;
        plan_side(&mut *guard, side, order, timestamp)?
    };
    if operations.is_empty() {
        return Ok(());
    }
    let directive = {
        let mut guard = state.lock().await;
        OrderDirective::new(guard.next_directive_id(), operations)
    };
    directive_tx
        .send(directive)
        .await
        .map_err(|_| anyhow!("directive channel closed"))?;
    Ok(())
}

fn plan_cancel_all(
    state: &mut ExecutionState,
    reason: &str,
    _timestamp: Instant,
) -> Vec<OrderDirective> {
    let mut operations = Vec::new();
    let live_indices: Vec<_> = state.live_by_order.keys().copied().collect();
    for order_index in live_indices {
        if let Some(live) = state.live_by_order.get(&order_index).cloned() {
            let already_pending = state
                .pending_by_client
                .get(&live.client_order_id)
                .map(|pending| matches!(pending.action, PendingAction::Cancel { .. }))
                .unwrap_or(false);
            if already_pending {
                debug!(
                    "Skipping duplicate cancel for order {} (already pending)",
                    order_index
                );
                continue;
            }
            let operation = OrderOperation::Cancel(OrderCancel {
                side: live.side,
                order_index: live.order_index,
                client_order_id: live.client_order_id,
            });
            operations.push(operation);
        }
    }
    if operations.is_empty() {
        debug!("cancel_all requested ({reason}) but no live orders or all already pending cancel");
        return Vec::new();
    }
    let id = state.next_directive_id();
    info!(
        "Issuing targeted cancel-all for {} orders ({})",
        operations.len(),
        reason
    );
    vec![OrderDirective::new(id, operations)]
}

fn plan_side(
    state: &mut ExecutionState,
    side: OrderSide,
    order: Option<QuoteOrder>,
    now: Instant,
) -> Result<Vec<OrderOperation>> {
    let mut operations = Vec::new();

    let target_ticks = order.as_ref().map(|o| state.to_ticks(o.price, side));
    {
        let slot = state.side_slot_mut(side);
        slot.desired_ticks = target_ticks;
        slot.desired_updated_at = Some(now);
    }

    let allow_refresh = state.compute_refresh_guard(side, now);
    let live_index = state
        .per_side
        .get(&side)
        .and_then(|slot| slot.live_order_index);
    let live_snapshot = live_index.and_then(|idx| state.live_by_order.get(&idx).cloned());

    if let (Some(target_ticks), Some(ref live)) = (target_ticks, live_snapshot.as_ref()) {
        let diff = (live.price_ticks - target_ticks).abs();
        if diff <= state.refresh_tolerance_ticks && allow_refresh {
            debug!(
                "[{}] target {} ticks within tolerance (Δ{}), leaving order live",
                side.as_str(),
                target_ticks,
                diff
            );
            return Ok(operations);
        }
    }

    let mut cancel_required = Vec::new();
    if order.is_none() {
        if let Some(live) = live_snapshot.clone() {
            cancel_required.push(live);
        }
    } else if let (Some(live), Some(target_ticks)) = (live_snapshot.clone(), target_ticks) {
        if (live.price_ticks - target_ticks).abs() > state.refresh_tolerance_ticks {
            cancel_required.push(live);
        }
    }

    for live in cancel_required {
        let version = state
            .per_side
            .get(&side)
            .map(|slot| slot.latest_version)
            .unwrap_or_default();
        let client_id = state.register_pending(
            side,
            version,
            PendingAction::Cancel {
                order_index: live.order_index,
            },
            Some(live.client_order_id),
        );
        debug!(
            "[{}] scheduling cancel for order {} (client {})",
            side.as_str(),
            live.order_index,
            client_id
        );
        operations.push(OrderOperation::Cancel(OrderCancel {
            side,
            order_index: live.order_index,
            client_order_id: client_id,
        }));
    }

    if let Some(order) = order {
        let target_ticks = state.to_ticks(order.price, side);
        let pending_ids = state
            .per_side
            .get(&side)
            .map(|slot| slot.pending_creates.clone())
            .unwrap_or_default();
        let pending_matches_target = pending_ids.iter().any(|client_id| {
            state
                .pending_by_client
                .get(client_id)
                .and_then(|pending| pending.action.price_ticks())
                .map(|ticks| ticks == target_ticks)
                .unwrap_or(false)
        });

        if pending_matches_target {
            debug!(
                "[{}] pending order already targeting {} ticks; skipping placement",
                side.as_str(),
                target_ticks
            );
            return Ok(operations);
        }

        let version = state.side_slot_mut(side).bump_version();
        let client_id = state.register_pending(
            side,
            version,
            PendingAction::Create {
                price_ticks: target_ticks,
            },
            None,
        );
        debug!(
            "[{}] scheduling place for client {} at {} ticks (v={})",
            side.as_str(),
            client_id,
            target_ticks,
            version
        );
        operations.push(OrderOperation::Place(OrderPlacement {
            side,
            price_ticks: target_ticks,
            client_order_id: client_id,
        }));
    }

    state.log_live_order_summary();
    Ok(operations)
}

fn spawn_signing_loop(
    mut directive_rx: mpsc::Receiver<OrderDirective>,
    signed_tx: mpsc::Sender<SignedDirective>,
    state: Arc<Mutex<ExecutionState>>,
    client: Arc<LighterClient>,
) {
    tokio::spawn(async move {
        while let Some(directive) = directive_rx.recv().await {
            if let Err(err) =
                sign_directive(state.clone(), Arc::clone(&client), &signed_tx, directive).await
            {
                error!("Signing pipeline failed: {err:#}");
            }
        }
    });
}

async fn sign_directive(
    state: Arc<Mutex<ExecutionState>>,
    client: Arc<LighterClient>,
    signed_tx: &mpsc::Sender<SignedDirective>,
    directive: OrderDirective,
) -> Result<()> {
    let mut signed_ops = Vec::with_capacity(directive.operations.len());
    for operation in &directive.operations {
        match operation {
            OrderOperation::Place(place) => {
                let payload = sign_place(&client, place, state.clone())
                    .await
                    .with_context(|| format!("sign place {}", place.client_order_id))?;
                {
                    let mut guard = state.lock().await;
                    guard.track_signed_signature(
                        place.client_order_id,
                        payload.nonce,
                        payload.api_key_index,
                    );
                }
                signed_ops.push(SignedOperation::new(
                    operation.clone(),
                    payload.entry,
                    TX_TYPE_CREATE_ORDER,
                    payload.api_key_index,
                ));
            }
            OrderOperation::Cancel(cancel) => {
                let payload = sign_cancel(&client, cancel, state.clone())
                    .await
                    .with_context(|| format!("sign cancel {}", cancel.client_order_id))?;
                {
                    let mut guard = state.lock().await;
                    guard.track_signed_signature(
                        cancel.client_order_id,
                        payload.nonce,
                        payload.api_key_index,
                    );
                }
                signed_ops.push(SignedOperation::new(
                    operation.clone(),
                    payload.entry,
                    TX_TYPE_CANCEL_ORDER,
                    payload.api_key_index,
                ));
            }
        }
    }

    if signed_ops.is_empty() {
        return Ok(());
    }

    signed_tx
        .send(SignedDirective::new(
            directive.id,
            directive.attempt,
            signed_ops,
        ))
        .await
        .map_err(|_| anyhow!("signed directive channel closed"))
}

struct SignedPayloadMeta {
    entry: BatchEntry,
    nonce: Option<i64>,
    api_key_index: Option<i32>,
}

async fn sign_place(
    client: &Arc<LighterClient>,
    place: &OrderPlacement,
    state: Arc<Mutex<ExecutionState>>,
) -> Result<SignedPayloadMeta> {
    let guard = state.lock().await;
    let qty = guard.base_qty;
    let market = guard.market;
    drop(guard);

    let signer = client
        .signer()
        .context("client not configured with signer (missing private key)?")?;
    let (api_key, nonce) = signer.next_nonce().await?;

    let builder = if place.side.is_ask() {
        client
            .order(market)
            .sell()
            .qty(qty)
            .limit(Price::ticks(place.price_ticks))
            .post_only()
            .with_client_order_id(place.client_order_id)
    } else {
        client
            .order(market)
            .buy()
            .qty(qty)
            .limit(Price::ticks(place.price_ticks))
            .post_only()
            .with_client_order_id(place.client_order_id)
    }
    .with_api_key(ApiKeyIndex::new(api_key))
    .with_nonce(Nonce::new(nonce));

    let signed = builder.sign().await?;
    let (entry, parsed) = signed.into_parts();
    let nonce = parsed.nonce;
    Ok(SignedPayloadMeta {
        entry,
        nonce,
        api_key_index: Some(api_key),
    })
}

async fn sign_cancel(
    client: &Arc<LighterClient>,
    cancel: &OrderCancel,
    state: Arc<Mutex<ExecutionState>>,
) -> Result<SignedPayloadMeta> {
    let market = {
        let guard = state.lock().await;
        guard.market
    };
    let signer = client
        .signer()
        .context("client configured without signer - cannot cancel")?;
    let (api_key, nonce) = signer.next_nonce().await?;
    let signed = signer
        .sign_cancel_order(
            market.into_inner(),
            cancel.order_index as i64,
            Some(nonce),
            Some(api_key),
        )
        .await?;

    let (entry, parsed) = signed.into_parts();
    let nonce = parsed.nonce;
    Ok(SignedPayloadMeta {
        entry,
        nonce,
        api_key_index: Some(api_key),
    })
}

fn spawn_submission_loop(
    mut signed_rx: mpsc::Receiver<SignedDirective>,
    directive_tx: mpsc::Sender<OrderDirective>,
    state: Arc<Mutex<ExecutionState>>,
    connection: Arc<Mutex<WsConnection>>,
) {
    tokio::spawn(async move {
        while let Some(batch) = signed_rx.recv().await {
            if let Err(err) = submit_signed_batch(
                batch,
                directive_tx.clone(),
                state.clone(),
                connection.clone(),
            )
            .await
            {
                error!("Submission pipeline error: {err:#}");
            }
        }
    });
}

async fn submit_signed_batch(
    batch: SignedDirective,
    directive_tx: mpsc::Sender<OrderDirective>,
    state: Arc<Mutex<ExecutionState>>,
    connection: Arc<Mutex<WsConnection>>,
) -> Result<()> {
    if batch.operations.is_empty() {
        return Ok(());
    }

    {
        let state_guard = state.lock().await;
        if state_guard.dry_run {
            drop(state_guard);
            info!(
                "DRY-RUN | would submit directive {} with {} ops",
                batch.id,
                batch.operations.len()
            );
            return Ok(());
        }
    }

    let mut ws = connection.lock().await;
    let txs: Vec<(u8, String)> = batch
        .operations
        .iter()
        .map(|op| (op.tx_type, op.batch_entry.tx_info().to_string()))
        .collect();

    let mode = {
        let guard = state.lock().await;
        if guard.fast_execution && guard.optimistic_acks {
            BatchAckMode::Optimistic {
                wait_timeout: Duration::from_millis(FAST_MODE_TIMEOUT_MS),
            }
        } else {
            BatchAckMode::Strict
        }
    };

    let send_result = send_batch_tx_ws_with_mode(&mut ws, txs, mode).await;
    drop(ws);
    match send_result {
        Ok(responses) => {
            handle_batch_result(state.clone(), directive_tx, batch, responses).await?;
        }
        Err(err) => {
            warn!("Batch submission failed: {err}");
            handle_batch_failure(state, directive_tx, batch).await?;
        }
    }

    Ok(())
}

async fn handle_batch_result(
    state: Arc<Mutex<ExecutionState>>,
    directive_tx: mpsc::Sender<OrderDirective>,
    batch: SignedDirective,
    results: Vec<bool>,
) -> Result<()> {
    let mut failures = Vec::new();
    for (idx, ok) in results.into_iter().enumerate() {
        let op = batch
            .operations
            .get(idx)
            .context("result length mismatch")?;
        let side = op.operation.side();
        let client_id = match &op.operation {
            OrderOperation::Place(place) => place.client_order_id,
            OrderOperation::Cancel(cancel) => cancel.client_order_id,
        };

        if ok {
            let mut guard = state.lock().await;
            guard.record_submission(side, Instant::now());
        } else {
            failures.push(client_id);
        }
    }

    if failures.is_empty() {
        return Ok(());
    }

    let mut retries = Vec::new();
    for client_id in failures {
        let mut guard = state.lock().await;
        if let Some(pending) = guard.record_submission_failure(client_id) {
            retries.push((client_id, pending));
        }
    }

    if retries.is_empty() {
        return Ok(());
    }

    for (_client_id, pending) in &retries {
        if let Some(nonce) = pending.nonce {
            let _ = state.lock().await.pending_by_nonce.remove(&nonce);
        }
    }

    let retry_ops: Vec<OrderOperation> = retries
        .into_iter()
        .filter_map(|(_, pending)| match pending.action {
            PendingAction::Create { price_ticks } => Some(OrderOperation::Place(OrderPlacement {
                side: pending.side,
                price_ticks,
                client_order_id: pending.client_order_id,
            })),
            PendingAction::Cancel { order_index } => Some(OrderOperation::Cancel(OrderCancel {
                side: pending.side,
                order_index,
                client_order_id: pending.client_order_id,
            })),
        })
        .collect();

    if retry_ops.is_empty() {
        return Ok(());
    }

    let max_backoff_ms = {
        let guard = state.lock().await;
        retry_ops
            .iter()
            .filter_map(|op| match op {
                OrderOperation::Place(place) => guard
                    .pending_by_client
                    .get(&place.client_order_id)
                    .map(|p| p.backoff_ms),
                OrderOperation::Cancel(cancel) => guard
                    .pending_by_client
                    .get(&cancel.client_order_id)
                    .map(|p| p.backoff_ms),
            })
            .max()
            .unwrap_or(INITIAL_BACKOFF_MS)
    };
    let backoff = Duration::from_millis(max_backoff_ms);

    let id = { state.lock().await.next_directive_id() };
    let directive = OrderDirective::new(id, retry_ops).with_attempt(batch.attempt + 1);
    tokio::spawn(async move {
        sleep(backoff).await;
        if let Err(err) = directive_tx.send(directive).await {
            error!("Failed to requeue directive after backoff: {err}");
        }
    });

    Ok(())
}

async fn handle_batch_failure(
    state: Arc<Mutex<ExecutionState>>,
    directive_tx: mpsc::Sender<OrderDirective>,
    batch: SignedDirective,
) -> Result<()> {
    let mut retries = Vec::new();
    for op in &batch.operations {
        let client_id = match &op.operation {
            OrderOperation::Place(place) => place.client_order_id,
            OrderOperation::Cancel(cancel) => cancel.client_order_id,
        };
        let mut guard = state.lock().await;
        if let Some(pending) = guard.record_submission_failure(client_id) {
            retries.push((client_id, pending));
        }
    }

    if retries.is_empty() {
        return Ok(());
    }

    for (_, pending) in &retries {
        if let Some(nonce) = pending.nonce {
            let _ = state.lock().await.pending_by_nonce.remove(&nonce);
        }
    }

    let retry_ops: Vec<OrderOperation> = retries
        .into_iter()
        .filter_map(|(_, pending)| match pending.action {
            PendingAction::Create { price_ticks } => Some(OrderOperation::Place(OrderPlacement {
                side: pending.side,
                price_ticks,
                client_order_id: pending.client_order_id,
            })),
            PendingAction::Cancel { order_index } => Some(OrderOperation::Cancel(OrderCancel {
                side: pending.side,
                order_index,
                client_order_id: pending.client_order_id,
            })),
        })
        .collect();

    if retry_ops.is_empty() {
        return Ok(());
    }

    let max_backoff_ms = {
        let guard = state.lock().await;
        retry_ops
            .iter()
            .filter_map(|op| match op {
                OrderOperation::Place(place) => guard
                    .pending_by_client
                    .get(&place.client_order_id)
                    .map(|p| p.backoff_ms),
                OrderOperation::Cancel(cancel) => guard
                    .pending_by_client
                    .get(&cancel.client_order_id)
                    .map(|p| p.backoff_ms),
            })
            .max()
            .unwrap_or(INITIAL_BACKOFF_MS)
    };
    let backoff = Duration::from_millis(max_backoff_ms);

    let id = { state.lock().await.next_directive_id() };
    let directive = OrderDirective::new(id, retry_ops).with_attempt(batch.attempt + 1);
    tokio::spawn(async move {
        sleep(backoff).await;
        if let Err(err) = directive_tx.send(directive).await {
            error!("Failed to requeue directive after submission failure: {err}");
        }
    });

    Ok(())
}

fn spawn_account_reconciliation_loop(
    mut account_rx: mpsc::Receiver<AccountOrdersUpdate>,
    directive_tx: mpsc::Sender<OrderDirective>,
    state: Arc<Mutex<ExecutionState>>,
) {
    tokio::spawn(async move {
        while let Some(update) = account_rx.recv().await {
            if let Err(err) =
                reconcile_account_orders(state.clone(), directive_tx.clone(), update).await
            {
                warn!("Account reconciliation failed: {err:#}");
            }
        }
    });
}

async fn reconcile_account_orders(
    state: Arc<Mutex<ExecutionState>>,
    directive_tx: mpsc::Sender<OrderDirective>,
    update: AccountOrdersUpdate,
) -> Result<()> {
    let market = { state.lock().await.market };
    let orders = extract_orders(update.payload, market)?;
    if orders.is_empty() {
        return Ok(());
    }

    let directives: Vec<OrderDirective> = Vec::new();
    let target_market = market.into_inner() as u32;
    {
        let mut guard = state.lock().await;
        for order in orders {
            if let Some(market_idx) = order.market_index {
                if market_idx != target_market {
                    continue;
                }
            }
            if let (Some(order_index), Some(client_id)) =
                (order.order_index, order.client_order_index)
            {
                let side = order
                    .is_ask
                    .map(OrderSide::from_is_ask)
                    .unwrap_or(OrderSide::Ask);
                let price_ticks = order
                    .price
                    .as_deref()
                    .and_then(|p| p.parse::<f64>().ok())
                    .map(|price| guard.to_ticks(price, side));

                let status = order
                    .status
                    .as_deref()
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_default();

                match status.as_str() {
                    "open" | "partial" | "partially_filled" => {
                        let live = LiveOrder {
                            order_index,
                            client_order_id: client_id,
                            side,
                            price_ticks: price_ticks.unwrap_or_default(),
                        };
                        guard.mark_live(live);
                    }
                    "filled" | "cancelled" | "canceled" => {
                        guard.remove_live_by_index(order_index);
                        guard.remove_live_by_client(client_id);
                    }
                    _ => {}
                }

                let mut clear_pending_create = false;
                let mut set_live_index = false;
                let mut clear_pending_cancel = false;
                let mut remove_pending_entry = false;

                if let Some(pending) = guard.pending_by_client.get_mut(&client_id) {
                    if price_ticks.is_some()
                        && matches!(pending.action, PendingAction::Create { .. })
                    {
                        pending.set_nonce(None);
                        pending.superseded = false;
                        pending.backoff_ms = INITIAL_BACKOFF_MS;
                        pending.retry_budget = MAX_RETRY_ATTEMPTS;
                        clear_pending_create = true;
                        set_live_index = true;
                    }

                    if matches!(pending.action, PendingAction::Cancel { .. }) {
                        clear_pending_cancel = true;
                        remove_pending_entry = true;
                    }
                }

                if clear_pending_create {
                    guard.side_slot_mut(side).clear_pending_create(client_id);
                }
                if set_live_index {
                    guard.side_slot_mut(side).live_order_index = Some(order_index);
                }
                if clear_pending_cancel {
                    guard.side_slot_mut(side).clear_pending_cancel(client_id);
                }
                if remove_pending_entry {
                    guard.pending_by_client.remove(&client_id);
                }
            }
        }
    }

    for directive in directives {
        directive_tx
            .send(directive)
            .await
            .map_err(|_| anyhow!("directive channel closed"))?;
    }

    Ok(())
}

fn extract_orders(value: Value, market: MarketId) -> Result<Vec<AccountOrderSnapshot>> {
    let mut rows = Vec::new();
    if let Some(node) = value.get("orders") {
        if let Some(arr) = node.as_array() {
            rows.extend(arr.iter().cloned());
        } else if let Some(obj) = node.as_object() {
            let key = market.into_inner().to_string();
            if let Some(entry) = obj.get(&key) {
                if let Some(arr) = entry.as_array() {
                    rows.extend(arr.iter().cloned());
                }
            } else {
                for entry in obj.values() {
                    if let Some(arr) = entry.as_array() {
                        rows.extend(arr.iter().cloned());
                    }
                }
            }
        }
    }
    let mut result = Vec::with_capacity(rows.len());
    for order in rows {
        let parsed: AccountOrderSnapshot = serde_json::from_value(order)?;
        result.push(parsed);
    }
    Ok(result)
}

fn spawn_ack_loop(
    mut ack_rx: mpsc::Receiver<Vec<TransactionData>>,
    directive_tx: mpsc::Sender<OrderDirective>,
    state: Arc<Mutex<ExecutionState>>,
) {
    tokio::spawn(async move {
        while let Some(txs) = ack_rx.recv().await {
            if let Err(err) =
                process_transaction_ack(state.clone(), directive_tx.clone(), txs).await
            {
                warn!("Transaction ack processing failed: {err:#}");
            }
        }
    });
}

async fn process_transaction_ack(
    state: Arc<Mutex<ExecutionState>>,
    directive_tx: mpsc::Sender<OrderDirective>,
    txs: Vec<TransactionData>,
) -> Result<()> {
    for tx in txs {
        let nonce = tx.nonce as i64;
        let mut guard = state.lock().await;
        if let Some(entry) = guard.pending_by_nonce.remove(&nonce) {
            let client_id = entry.client_id;
            if tx.status != 1 {
                warn!(
                    "Transaction failure for nonce {} (client {}) status={}",
                    nonce, client_id, tx.status
                );
                if let Some(pending) = guard.record_submission_failure(client_id) {
                    match pending.action {
                        PendingAction::Create { price_ticks } => {
                            if pending.exhausted() {
                                debug!(
                                    "Dropping exhausted CREATE operation for client {}",
                                    client_id
                                );
                                continue;
                            }
                            let op = OrderOperation::Place(OrderPlacement {
                                side: pending.side,
                                price_ticks,
                                client_order_id: pending.client_order_id,
                            });
                            let id = guard.next_directive_id();
                            let directive = OrderDirective::new(id, vec![op])
                                .with_attempt(pending.attempts + 1);
                            drop(guard);
                            directive_tx
                                .send(directive)
                                .await
                                .map_err(|_| anyhow!("directive channel closed"))?;
                            continue;
                        }
                        PendingAction::Cancel { order_index } => {
                            debug!(
                                "Cancel failed for order {}, skipping retry (emergency breaker will handle)",
                                order_index
                            );
                            guard
                                .side_slot_mut(pending.side)
                                .clear_pending_cancel(client_id);
                            continue;
                        }
                    }
                }
            } else {
                if let Some(order) = guard.pending_by_client.remove(&client_id) {
                    match order.action {
                        PendingAction::Create { .. } => {
                            guard
                                .side_slot_mut(order.side)
                                .clear_pending_create(client_id);
                        }
                        PendingAction::Cancel { .. } => {
                            guard
                                .side_slot_mut(order.side)
                                .clear_pending_cancel(client_id);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn spawn_control_loop(
    mut control_rx: mpsc::Receiver<ControlCommand>,
    connection: Arc<Mutex<WsConnection>>,
    client: Arc<LighterClient>,
    state: Arc<Mutex<ExecutionState>>,
    auth_token: Arc<Mutex<Option<String>>>,
) {
    tokio::spawn(async move {
        while let Some(cmd) = control_rx.recv().await {
            match cmd {
                ControlCommand::Reconnect(reply) => {
                    let result = recover_transactions(
                        connection.clone(),
                        client.clone(),
                        state.clone(),
                        auth_token.clone(),
                    )
                    .await;
                    let _ = reply.send(result);
                }
            }
        }
    });
}

async fn recover_transactions(
    connection: Arc<Mutex<WsConnection>>,
    client: Arc<LighterClient>,
    _state: Arc<Mutex<ExecutionState>>,
    auth_token: Arc<Mutex<Option<String>>>,
) -> Result<bool> {
    info!("Attempting transaction channel reconnect");

    {
        let mut ws = connection.lock().await;
        if let Err(err) = ws.reconnect(Some(3)).await {
            warn!("Direct reconnect failed: {err}");
        } else {
            info!("Transaction websocket reconnected");
            return Ok(true);
        }
    }

    let new_token = client
        .create_auth_token(None)
        .context("failed to mint auth token for reconnect")?;

    {
        let mut guard = auth_token.lock().await;
        *guard = Some(new_token.clone());
    }

    let mut stream = client
        .ws()
        .connect()
        .await
        .context("failed to open fresh connection stream")?;
    stream.connection_mut().set_auth_token(new_token);
    let new_connection = stream.into_connection();

    {
        let mut ws = connection.lock().await;
        *ws = new_connection;
    }

    info!("Transaction websocket reconnected via fresh stream");
    Ok(true)
}
