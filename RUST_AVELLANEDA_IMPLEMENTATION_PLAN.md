# Rust Avellaneda Market Making Strategy – Implementation Roadmap

Objective: deliver a clean, formula-driven Avellaneda-Stoikov market maker in Rust that integrates with the Lighter SDK, reaches production readiness within three weeks, and remains maintainable at ~800 lines of core strategy code.

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Project Structure](#2-project-structure)
3. [Inventory Module](#3-inventory-module)
4. [Volatility Module](#4-volatility-module)
5. [Market Data Module](#5-market-data-module)
6. [Spread Optimizer Module](#6-spread-optimizer-module)
7. [Execution Module](#7-execution-module)
8. [Types Module](#8-types-module)
9. [Config Module](#9-config-module)
10. [Data Structures](#10-data-structures)
11. [Formula Implementation](#11-formula-implementation)
12. [Lighter SDK Integration](#12-lighter-sdk-integration)
13. [Implementation Phases](#13-implementation-phases)
14. [Testing Strategy](#14-testing-strategy)
15. [Performance Targets](#15-performance-targets)
16. [Risk Controls](#16-risk-controls)
17. [Deployment Plan](#17-deployment-plan)

---

## 1. Architecture Overview

**Design Goals**
- Maintain strict module boundaries: data ingestion → state → pricing → execution.
- Keep the hot path (market data → quotes) deterministic and allocation-free.
- Surface all tunables through `config.toml` with five core parameters (`gamma`, `kappa`, `time_horizon_hours`, `target_base_pct`, `vol_lookback`) and minimal safety bounds.
- Reuse the Lighter SDK for connectivity, signing, and matching engine semantics.

**Component Flow**
1. Market data adapter streams top-of-book updates into a lock-free ring buffer.
2. Volatility estimator maintains rolling EWMA using `ringbuffer`.
3. Inventory calculator normalizes position using up-to-date balances.
4. Spread optimizer derives reservation price and optimal spread from formulas.
5. Execution coordinator submits/refreshes quotes through Lighter SDK.
6. Risk controller wraps execution with pre-trade checks and circuit breakers.

**Concurrency Model**
- Single strategy task runs on Tokio, processing market ticks sequentially.
- Background tasks refresh balances and configuration at slower cadence.
- Quote publication occurs on the same async task to avoid race conditions.

---

## 2. Project Structure

```
Rust_SDK/
├── Cargo.toml
├── config.toml                  # Strategy parameters & environment
├── src/
│   ├── avellaneda/
│   │   ├── mod.rs               # Public interface: AvellanedaStrategy
│   │   ├── inventory.rs         # Normalized inventory logic
│   │   ├── volatility.rs        # Rolling EWMA volatility
│   │   ├── market_data.rs       # Lighter streams → internal structs
│   │   ├── spreads.rs           # Reservation price & spread math
│   │   ├── execution.rs         # Order management & fills
│   │   ├── types.rs             # Domain structs/enums
│   │   └── config.rs            # Loading + validation of config
│   └── sdk.rs                   # Thin wrappers around Lighter SDK (if needed)
├── examples/trading/avellaneda/
│   └── main.rs                  # Launch entrypoint for manual runs
├── tests/
│   ├── formulas.rs              # Unit tests for math
│   ├── integration.rs           # End-to-end quote assertions
│   └── backtests.rs             # Historical replay harness (Phase 7)
└── docs/
    ├── QUICKSTART.md
    ├── FORMULAS.md
    ├── STRATEGY_COMPARISON.md
    └── RUST_AVELLANEDA_IMPLEMENTATION_PLAN.md
```

---

## 3. Inventory Module

**Responsibilities**
- Track real-time balances from Lighter (base/quote).
- Compute normalized inventory `q = (actual - target) / total`.
- Provide target base inventory based on `target_base_pct`.
- Expose helper to clamp inventory within `max_position`.

**Key Functions**
- `InventoryState::update_balances(mid_price, base_balance, quote_balance)`
- `InventoryState::normalized_inventory() -> f64`
- `InventoryState::should_reduce(long_limit, short_limit) -> bool`

**Implementation Notes**
- Use lightweight struct with cached mid-price to avoid redundant conversions.
- Maintain last fill timestamps to coordinate with execution throttles.
- Serialize state snapshots for telemetry (optional feature flag).

---

## 4. Volatility Module

**Responsibilities**
- Maintain price sample buffer (`ringbuffer::AllocRingBuffer`).
- Compute log returns and EWMA volatility (`statrs` for math utilities).
- Produce annualized sigma and instantaneous sigma for diagnostics.

**Key Functions**
- `VolEstimator::new(lookback: usize, alpha: f64)`
- `VolEstimator::on_mid_price(mid: f64, timestamp: Instant)`
- `VolEstimator::sigma_annualized() -> f64`
- `VolEstimator::sigma_short_term(window_secs: f64) -> f64`

**Implementation Notes**
- Use monotonic timestamps; discard samples older than `lookback`.
- Provide `VolSnapshot` struct for logging/testing.
- Clamp minimum sigma to avoid collapsing spreads (< 10 bps).

---

## 5. Market Data Module

**Responsibilities**
- Subscribe to order book and trade streams via Lighter SDK.
- Convert raw messages into `MarketTick { mid, bid, ask, spread, timestamp }`.
- Maintain smoothing of mid-price using micro EWMA to reduce noise.
- Handle connection resilience (auto-resubscribe, heartbeat checks).

**Key Functions**
- `MarketDataStream::subscribe(market_id: u64) -> impl Stream<Item = MarketTick>`
- `MarketDataStream::last_tick() -> Option<MarketTick>`
- `MarketDataStream::on_disconnect()` for recovery hooks.

**Implementation Notes**
- Use Lighter WebSocket client; integrate `tokio::select!` for reconnection.
- Rate-limit logging to once per second to keep console clean.
- Emit metrics counters for stale data detection.

---

## 6. Spread Optimizer Module

**Responsibilities**
- Implement Avellaneda-Stoikov formulas for reservation price and spreads.
- Apply safety clamps (`min_spread_bps`, `max_spread_bps`).
- Adjust spread for maker fees and tick size rounding.

**Key Functions**
- `SpreadCalculator::reservation_price(mid, q, gamma, sigma, time_left)`
- `SpreadCalculator::optimal_spread(gamma, sigma, time_left, kappa)`
- `SpreadCalculator::quotes(mid, q, sigma, params) -> QuotePair`

**Implementation Notes**
- Pre-compute `gamma_terms` to avoid repeated multiplications in hot path.
- Provide deterministic rounding helper to align with exchange tick size.
- Return both theoretical (`r`, `delta`) and practical (bid/ask) outputs.

---

## 7. Execution Module

**Responsibilities**
- Manage working orders: create, amend, cancel.
- Enforce refresh cadence (200 ms default).
- Coordinate with inventory flags to skew sizes if needed.
- Record fills and feed back into inventory state.

**Key Functions**
- `ExecutionEngine::submit_quotes(QuotePair, inventory_state)`
- `ExecutionEngine::handle_fill(fill: FillEvent)`
- `ExecutionEngine::cancel_all(reason: CancelReason)`

**Implementation Notes**
- Use Lighter `OrderBuilder` for simplicity; support dry-run mode.
- Maintain per-side order IDs for idempotent updates.
- Emit `ExecutionEvent` enum for logging/testing hooks.

---

## 8. Types Module

**Responsibilities**
- Centralize shared Rust structs/enums and trait definitions.
- Provide `StrategyParams`, `QuotePair`, `InventorySnapshot`, etc.
- Implement serde derives for config serialization.

**Key Structures**
- `pub struct StrategyParams { gamma: f64, kappa: f64, ... }`
- `pub struct QuotePair { pub bid: QuoteOrder, pub ask: QuoteOrder }`
- `pub struct QuoteOrder { pub price: f64, pub size: f64 }`
- `pub enum StrategyEvent { Tick(MarketTick), Quote(QuotePair), Fill(FillEvent), ... }`

**Implementation Notes**
- Keep structs `#[derive(Clone, Debug)]` for cheap logging/testing.
- Provide conversion traits from SDK types into local types.
- Add `TimeHorizon` newtype to keep units explicit (seconds vs hours).

---

## 9. Config Module

**Responsibilities**
- Load `config.toml`, merge with environment overrides.
- Validate parameter ranges and apply sensible defaults.
- Expose strongly typed config struct for the strategy.

**Key Functions**
- `Config::from_toml(path) -> Result<AvellanedaConfig>`
- `AvellanedaConfig::core_params() -> StrategyParams`
- `AvellanedaConfig::safety_bounds() -> SafetyBounds`

**Implementation Notes**
- Validate: `0.01 <= gamma <= 1.0`, `0.5 <= kappa <= 3.0`, `0 < vol_lookback <= 500`.
- Support environment overrides (`AVELLANEDA_*`) for ops convenience.
- Provide `DryRunMode` flag toggled by `AVELLANEDA_DRY_RUN`.

---

## 10. Data Structures

Key Rust structs and their fields (non-exhaustive but required for compilation):

```rust
pub struct MarketTick {
    pub bid: f64,
    pub ask: f64,
    pub mid: f64,
    pub spread: f64,
    pub timestamp: Instant,
}

pub struct InventoryState {
    pub base_balance: f64,
    pub quote_balance: f64,
    pub mid_price: f64,
    pub target_pct: f64,
    pub max_position: f64,
}

pub struct VolEstimator {
    prices: AllocRingBuffer<f64>,
    alpha: f64,
    sigma: f64,
}

pub struct QuotePair {
    pub bid: QuoteOrder,
    pub ask: QuoteOrder,
    pub reservation_price: f64,
    pub optimal_spread: f64,
}

pub struct QuoteOrder {
    pub price: f64,
    pub size: f64,
    pub client_order_id: String,
}

pub struct AvellanedaConfig {
    pub market_id: u64,
    pub order_size: f64,
    pub gamma: f64,
    pub kappa: f64,
    pub time_horizon_hours: f64,
    pub target_base_pct: f64,
    pub vol_lookback: usize,
    pub vol_ewma_alpha: f64,
    pub refresh_interval_ms: u64,
    pub min_spread_bps: f64,
    pub max_spread_bps: f64,
    pub max_position: f64,
    pub min_notional: f64,
}

pub struct SafetyBounds {
    pub min_spread: f64,
    pub max_spread: f64,
    pub max_position: f64,
    pub volatility_breaker: f64,
}
```

---

## 11. Formula Implementation

All six core formulas with step-by-step logic and Rust-ready snippets.

**1. Normalized Inventory**
```
q = (B_actual - B_target) / B_total
```

```rust
pub fn normalized_inventory(state: &InventoryState) -> f64 {
    let total_value = state.base_balance * state.mid_price + state.quote_balance;
    if total_value <= f64::EPSILON {
        return 0.0;
    }
    let target_value = total_value * state.target_pct;
    let target_base = target_value / state.mid_price;
    let total_base = total_value / state.mid_price;
    (state.base_balance - target_base) / total_base
}
```

**2. Time Horizon Conversion**
```
T = time_horizon_hours × 3600
t_elapsed = now - start
τ = max(T - t_elapsed, ε)
```

```rust
pub fn time_left_seconds(start: Instant, horizon_hours: f64) -> f64 {
    let total = horizon_hours * 3600.0;
    let elapsed = start.elapsed().as_secs_f64();
    (total - elapsed).max(0.01)
}
```

**3. EWMA Volatility**
```
σ_t = α · σ_sample + (1 - α) · σ_{t-1}
σ_sample = std(log_returns)
```

```rust
pub fn update_sigma(estimator: &mut VolEstimator, price: f64) {
    estimator.on_mid_price(price, Instant::now());
}
```

**4. Reservation Price**
```
r = S - q × γ × σ × τ
```

```rust
pub fn reservation_price(mid: f64, q: f64, gamma: f64, sigma: f64, time_left: f64) -> f64 {
    mid - q * gamma * sigma * time_left
}
```

**5. Optimal Spread**
```
δ = γ × σ × τ + (2/γ) × ln(1 + γ/κ)
```

```rust
pub fn optimal_spread(gamma: f64, sigma: f64, time_left: f64, kappa: f64) -> f64 {
    let risk_term = gamma * sigma * time_left;
    let liquidity_term = (2.0 / gamma) * (1.0 + gamma / kappa).ln();
    risk_term + liquidity_term
}
```

**6. Quote Prices**
```
bid = r - δ/2
ask = r + δ/2
```

```rust
pub fn make_quotes(
    mid: f64,
    q: f64,
    sigma: f64,
    params: &StrategyParams,
) -> QuotePair {
    let time_left = params.time_left_seconds();
    let r = reservation_price(mid, q, params.gamma, sigma, time_left);
    let delta = optimal_spread(params.gamma, sigma, time_left, params.kappa);
    let half = delta / 2.0;
    QuotePair {
        bid: QuoteOrder::new(r - half, params.order_size),
        ask: QuoteOrder::new(r + half, params.order_size),
        reservation_price: r,
        optimal_spread: delta,
    }
}
```

Include rounding and safety clamps post-calculation to respect tick size and spread bounds.

---

## 12. Lighter SDK Integration

- Use `lighter_sdk::ws::Client` for streaming top-of-book quotes.
- Use `lighter_sdk::rest::Orders` for order placement (or WebSocket trading if available).
- Authentication handled via `signers/` keys; expose `SignerManager`.
- Map SDK types to local types in `types.rs`.
- Configure reconnect policy: exponential backoff up to 5 seconds.
- Enable dry-run by switching to `lighter_sdk::sandbox::Client`.
- Capture `ExecutionReport` events → update inventory and logs.

Sequence:
1. Initialize SDK session with credentials (Phase 3).
2. Subscribe to market data, spawn stream processing task.
3. For each tick, call `strategy.on_tick(tick)`.
4. Strategy emits `QuotePair` → `execution.submit_quotes()`.
5. Execution engine uses SDK to send `replace_order` when price changes.

---

## 13. Implementation Phases

Total duration: 14 days (2 weeks). Each phase maps to daily goals.

- **Phase 1 (Day 1-2)**: Implement `types.rs`, `config.rs`, scaffolding; unit tests for structs.
- **Phase 2 (Day 3-4)**: Implement formulas in `inventory.rs`, `volatility.rs`, `spreads.rs`; ensure tests pass.
- **Phase 3 (Day 5-6)**: Build `market_data.rs` streaming adapters; integrate with Lighter mock streams.
- **Phase 4 (Day 7-8)**: Implement `execution.rs` with dry-run mode, order refresh loop.
- **Phase 5 (Day 9-10)**: Wire inventory feedback loops, implement position limits, fill handling.
- **Phase 6 (Day 11-12)**: Add risk controls (spread bounds, volatility breaker, kill switch).
- **Phase 7 (Day 13-14)**: Comprehensive testing, historical backtest run, prepare deployment artifacts.

Buffer weekends for reviews/documentation if schedule slips.

---

## 14. Testing Strategy

- **Unit Tests**: math correctness (`tests/formulas.rs`), config validation, inventory normalization edge cases.
- **Integration Tests**: simulated market data stream; assert spread symmetry, risk clamps, order lifecycle.
- **Backtesting**: replay historical tick data (CSV) to ensure profitability envelope; run nightly on CI.
- **Property Tests**: ensure spreads never invert (`bid < ask`), invariants hold for parameter ranges.
- **Latency Benchmarks**: criterion-based benchmark for quote generation (< 50 µs per tick).
- **Smoke Tests**: dry-run sessions for 10 minutes before production deploys.

---

## 15. Performance Targets

- Quote computation latency: < 50 µs per tick (single-thread hot path).
- End-to-end tick-to-order latency: < 20 ms (including SDK overhead).
- CPU usage: < 1 core at 5 Hz quote refresh.
- Memory: < 150 MB RSS (majority in SDK + ring buffers).
- Market data throughput: handle 200 ticks/sec without lag.
- Logging overhead: < 5% CPU; use structured logging.

Monitor via Prometheus metrics or lightweight rolling averages printed every second.

---

## 16. Risk Controls

- **Pre-trade Checks**: ensure `order_size × price >= min_notional`; enforce position limits.
- **Volatility Circuit Breaker**: halt quoting if `sigma` > `volatility_breaker` (default 3× rolling median).
- **Kill Switch**: environment toggle or CLI signal to cancel all orders and exit gracefully.
- **Spread Guards**: clamp spreads between `min_spread_bps` and `max_spread_bps`.
- **Inventory Guard**: pause the heavy side when `|q| > 0.6` or position near limits.
- **Connectivity Watchdog**: cancel orders if market data stale > 1 sec or execution channel down.

All guards emit structured events for observability and post-mortem analysis.

---

## 17. Deployment Plan

1. **Canary (Day 13-14)**: dry-run + sandbox, then live with min size for 30 minutes; monitor metrics.
2. **Limited Rollout (Week 3 Days 15-17)**: trade with 10% target inventory, watch P&L, adjust params.
3. **Full Production (Week 3 Days 18-21)**: scale to full target balances, enable alerting, hand off to ops.

Operational Checklist:
- Capture baseline metrics (latency, fills, inventory drift).
- Review logs daily for first week.
- Schedule parameter tuning windows (gamma/kappa adjustments) with risk sign-off.
- Document rollback procedure (`execution.cancel_all()` + disable service).

```
┌─────────────────────────────────────────────────────────────┐
│                    MM Avellaneda Bot                        │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌──────────────┐  ┌──────────────┐  ┌─────────────────┐  │
│  │   Market     │  │   Strategy   │  │   Execution     │  │
│  │   Data       │→ │   Engine     │→ │   Manager       │  │
│  └──────────────┘  └──────────────┘  └─────────────────┘  │
│         ↓                 ↓                    ↓            │
│  ┌──────────────┐  ┌──────────────┐  ┌─────────────────┐  │
│  │  Volatility  │  │  Inventory   │  │   Order Book    │  │
│  │  Estimator   │  │  Tracker     │  │   Manager       │  │
│  └──────────────┘  └──────────────┘  └─────────────────┘  │
│         ↓                 ↓                    ↓            │
│  ┌──────────────────────────────────────────────────────┐  │
│  │           Lighter SDK (WebSocket + REST)             │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 1.2 Key Differences from mm_hawkes.rs

| Feature | mm_hawkes.rs | Avellaneda Strategy |
|---------|--------------|---------------------|
| **Pricing Model** | Hawkes toxicity + oscillation detection | Reservation price + optimal spread |
| **Inventory** | Same-side refill throttling | Normalized inventory skew |
| **Volatility** | 2s bar EWMA | Rolling window + EWMA |
| **Risk Model** | Crash detector + FlipGate | Risk aversion parameter (γ) |
| **Spreads** | Fixed + skew | Dynamically calculated from γ, σ, T, κ |
| **Complexity** | 1500+ lines | Target: ~800 lines |

---

## 2. Project Structure

### 2.1 Directory Layout

```
MM_Avellaneda/Rust_SDK/
├── Cargo.toml                    # Already exists
├── config.toml                   # Already exists
├── src/                          # SDK source (already exists)
├── examples/
│   └── trading/
│       └── avellaneda/
│           ├── mm_avellaneda.rs           # Main bot (NEW)
│           ├── mod.rs                      # Module exports
│           ├── config.rs                   # Config loader
│           ├── inventory.rs                # Inventory tracking
│           ├── volatility.rs               # Vol estimation
│           ├── spreads.rs                  # A-S formula impl
│           ├── execution.rs                # Order management
│           └── types.rs                    # Data structures
├── tests/
│   └── avellaneda_tests.rs       # Unit tests (NEW)
└── docs/
    ├── AVELLANEDA_FORMULAS.md    # Math reference (NEW)
    ├── CONFIG_GUIDE.md           # Config docs (NEW)
    └── PARAMETERS.md             # Param tuning guide (NEW)
```

### 2.2 Cargo.toml Additions

Add to existing `Cargo.toml`:

```toml
[[example]]
name = "mm_avellaneda"
path = "examples/trading/avellaneda/mm_avellaneda.rs"

[dependencies]
# Already have: serde, tokio, reqwest, alloy, etc.
# Need to add:
statrs = "0.17"          # Statistical functions
ringbuffer = "0.15"      # Efficient circular buffers for price history
```

---

## 3. Core Modules Design

### 3.1 Module: `inventory.rs`

**Purpose**: Track normalized inventory and target allocation

**Key Functions**:
```rust
pub struct InventoryTracker {
    base_balance: f64,
    quote_balance: f64,
    target_base_pct: f64,  // Default: 0.5 (50%)
}

impl InventoryTracker {
    pub fn new(target_base_pct: f64) -> Self;

    pub fn update_balances(&mut self, base: f64, quote: f64);

    /// Calculate normalized inventory q
    /// q = (actual_base - target_base) / total_portfolio_in_base
    pub fn normalized_inventory(&self, mid_price: f64) -> f64;

    /// Calculate target inventory in base units
    pub fn target_inventory(&self, mid_price: f64) -> f64;

    /// Get total portfolio value in quote currency
    pub fn portfolio_value(&self, mid_price: f64) -> f64;
}
```

**Implementation Details**:
- Uses Hummingbot's normalized approach: `q = (actual - target) / total`
- `target_base_pct` defaults to 0.5 (50% in base asset)
- Portfolio value = `base_balance * mid_price + quote_balance`
- Target inventory = `portfolio_value * target_base_pct / mid_price`

### 3.2 Module: `volatility.rs`

**Purpose**: Real-time volatility estimation using EWMA

**Key Functions**:
```rust
use ringbuffer::{RingBuffer, ConstGenericRingBuffer};

pub struct VolatilityEstimator {
    price_buffer: ConstGenericRingBuffer<f64, 1000>,  // Last 1000 prices
    lookback_window: usize,                            // Default: 100
    ewma_alpha: f64,                                   // Default: 0.1
    current_vol: f64,
}

impl VolatilityEstimator {
    pub fn new(lookback: usize, alpha: f64) -> Self;

    /// Add new price observation
    pub fn update(&mut self, price: f64, timestamp: u64);

    /// Calculate volatility using rolling window + EWMA
    pub fn current_volatility(&self) -> f64;

    /// Get standard deviation of recent returns
    fn calculate_std_dev(&self) -> f64;
}
```

**Implementation Details**:
- Uses log returns: `r_i = ln(P_i / P_{i-1})`
- Standard deviation: `σ = sqrt(Σ(r_i - μ)² / n)`
- EWMA update: `σ_new = α × σ_sample + (1 - α) × σ_old`
- Annualize if needed: `σ_annual = σ_tick × sqrt(ticks_per_year)`

### 3.3 Module: `spreads.rs`

**Purpose**: Calculate reservation price and optimal spread using A-S formulas

**Key Functions**:
```rust
pub struct AvellanedaStoikov {
    gamma: f64,      // Risk aversion (default: 0.1)
    kappa: f64,      // Liquidity parameter (default: 1.5)
    time_horizon: f64,  // T in seconds (default: 3600 = 1 hour)
}

impl AvellanedaStoikov {
    pub fn new(gamma: f64, kappa: f64, time_horizon: f64) -> Self;

    /// Calculate reservation price
    /// r = mid - q × γ × σ × (T - t)
    pub fn reservation_price(
        &self,
        mid_price: f64,
        normalized_q: f64,
        volatility: f64,
        time_left_fraction: f64,
    ) -> f64;

    /// Calculate optimal spread
    /// δ = γ × σ × T + (2/γ) × ln(1 + γ/κ)
    pub fn optimal_spread(
        &self,
        volatility: f64,
        time_left_fraction: f64,
    ) -> f64;

    /// Calculate bid and ask quotes
    pub fn calculate_quotes(
        &self,
        mid_price: f64,
        normalized_q: f64,
        volatility: f64,
        time_left_fraction: f64,
    ) -> (f64, f64);  // (bid, ask)
}
```

**Formula Details**:

**Reservation Price**:
```
r = S - q × γ × σ × (T - t)

Where:
- S = current mid price
- q = normalized inventory (-1 to +1 typically)
- γ = risk aversion (0.1 = moderate, 1.0 = high)
- σ = volatility (annualized std dev)
- T = total time horizon
- t = time elapsed
- (T - t) = time remaining fraction
```

**Optimal Spread**:
```
δ = γ × σ × (T - t) + (2/γ) × ln(1 + γ/κ)

Component 1: γ × σ × (T - t)
  - Risk-based component
  - Wider spread when high risk aversion or volatility

Component 2: (2/γ) × ln(1 + γ/κ)
  - Liquidity-based component
  - κ = arrival rate of market orders
  - Wider spread when low liquidity
```

**Final Quotes**:
```
bid = r - δ/2
ask = r + δ/2
```

### 3.4 Module: `execution.rs`

**Purpose**: Manage order lifecycle and quote refreshing

**Key Functions**:
```rust
use crate::lighter_client::{LighterClient, OrderSide};
use crate::signer::SignerClient;

pub struct ExecutionManager {
    client: LighterClient,
    signer: SignerClient,
    market_id: u32,
    current_bid_id: Option<u64>,
    current_ask_id: Option<u64>,
    min_refresh_interval: Duration,
    last_refresh: Instant,
}

impl ExecutionManager {
    pub fn new(
        client: LighterClient,
        signer: SignerClient,
        market_id: u32,
        refresh_interval_ms: u64,
    ) -> Self;

    /// Place or update quotes
    pub async fn refresh_quotes(
        &mut self,
        bid_price: f64,
        ask_price: f64,
        size: f64,
    ) -> Result<(), Box<dyn std::error::Error>>;

    /// Cancel all active orders
    pub async fn cancel_all(&mut self) -> Result<(), Box<dyn std::error::Error>>;

    /// Check if refresh interval elapsed
    pub fn can_refresh(&self) -> bool;
}
```

### 3.5 Module: `types.rs`

**Purpose**: Shared data structures

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct MarketState {
    pub mid_price: f64,
    pub best_bid: f64,
    pub best_ask: f64,
    pub spread_bps: f64,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct StrategyState {
    pub reservation_price: f64,
    pub optimal_spread: f64,
    pub bid_quote: f64,
    pub ask_quote: f64,
    pub normalized_inventory: f64,
    pub current_volatility: f64,
    pub time_left_fraction: f64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AvellanedaConfig {
    // Trading parameters
    pub market_id: u32,
    pub order_size: f64,

    // Avellaneda parameters
    pub gamma: f64,                // Risk aversion
    pub kappa: f64,                // Liquidity parameter
    pub time_horizon_hours: f64,   // Strategy time horizon
    pub target_base_pct: f64,      // Target inventory (0.5 = 50%)

    // Volatility estimation
    pub vol_lookback: usize,       // Price samples for vol
    pub vol_ewma_alpha: f64,       // EWMA decay

    // Execution
    pub refresh_interval_ms: u64,  // Quote refresh rate
    pub min_spread_bps: f64,       // Minimum spread (safety)
    pub max_spread_bps: f64,       // Maximum spread (safety)

    // Risk controls
    pub max_position: f64,         // Position limit
    pub min_notional: f64,         // Min order value
}

impl Default for AvellanedaConfig {
    fn default() -> Self {
        Self {
            market_id: 1,
            order_size: 0.001,
            gamma: 0.1,
            kappa: 1.5,
            time_horizon_hours: 1.0,
            target_base_pct: 0.5,
            vol_lookback: 100,
            vol_ewma_alpha: 0.1,
            refresh_interval_ms: 200,
            min_spread_bps: 5.0,
            max_spread_bps: 100.0,
            max_position: 0.01,
            min_notional: 10.0,
        }
    }
}
```

---

## 4. Data Structures

### 4.1 Main Bot State

```rust
pub struct AvellanedaBot {
    // Configuration
    config: AvellanedaConfig,

    // Core components
    inventory: InventoryTracker,
    volatility: VolatilityEstimator,
    spreads: AvellanedaStoikov,
    execution: ExecutionManager,

    // Market state
    market_state: MarketState,
    strategy_state: StrategyState,

    // Timing
    start_time: Instant,
    last_update: Instant,
}
```

---

## 5. Formula Implementation

### 5.1 Core Calculation Flow

```rust
impl AvellanedaBot {
    /// Main calculation pipeline
    pub async fn update_quotes(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Step 1: Get current market data
        self.update_market_state().await?;

        // Step 2: Update volatility estimate
        self.volatility.update(
            self.market_state.mid_price,
            self.market_state.timestamp,
        );
        let vol = self.volatility.current_volatility();

        // Step 3: Calculate normalized inventory
        let q = self.inventory.normalized_inventory(self.market_state.mid_price);

        // Step 4: Calculate time remaining fraction
        let elapsed = self.start_time.elapsed().as_secs_f64();
        let time_horizon_sec = self.config.time_horizon_hours * 3600.0;
        let time_left = (time_horizon_sec - elapsed) / time_horizon_sec;
        let time_left = time_left.max(0.1);  // Never go below 10%

        // Step 5: Calculate reservation price and optimal spread
        let (bid, ask) = self.spreads.calculate_quotes(
            self.market_state.mid_price,
            q,
            vol,
            time_left,
        );

        // Step 6: Apply safety bounds
        let bid = self.apply_min_max_spread(bid, "bid");
        let ask = self.apply_min_max_spread(ask, "ask");

        // Step 7: Validate and snap to tick size
        let (bid_str, ask_str) = self.validate_and_snap(bid, ask)?;

        // Step 8: Refresh orders if interval elapsed
        if self.execution.can_refresh() {
            self.execution.refresh_quotes(
                bid,
                ask,
                self.config.order_size,
            ).await?;
        }

        // Step 9: Update strategy state for logging
        self.strategy_state = StrategyState {
            reservation_price: self.spreads.reservation_price(
                self.market_state.mid_price,
                q,
                vol,
                time_left,
            ),
            optimal_spread: self.spreads.optimal_spread(vol, time_left),
            bid_quote: bid,
            ask_quote: ask,
            normalized_inventory: q,
            current_volatility: vol,
            time_left_fraction: time_left,
        };

        Ok(())
    }

    fn apply_min_max_spread(&self, quote: f64, side: &str) -> f64 {
        let mid = self.market_state.mid_price;
        let min_spread_abs = mid * self.config.min_spread_bps / 10000.0;
        let max_spread_abs = mid * self.config.max_spread_bps / 10000.0;

        if side == "bid" {
            let spread = mid - quote;
            if spread < min_spread_abs {
                mid - min_spread_abs
            } else if spread > max_spread_abs {
                mid - max_spread_abs
            } else {
                quote
            }
        } else {  // ask
            let spread = quote - mid;
            if spread < min_spread_abs {
                mid + min_spread_abs
            } else if spread > max_spread_abs {
                mid + max_spread_abs
            } else {
                quote
            }
        }
    }
}
```

### 5.2 Volatility Calculation (Detailed)

```rust
impl VolatilityEstimator {
    fn calculate_std_dev(&self) -> f64 {
        if self.price_buffer.len() < 2 {
            return 0.0;
        }

        // Extract recent prices
        let prices: Vec<f64> = self.price_buffer
            .iter()
            .take(self.lookback_window.min(self.price_buffer.len()))
            .copied()
            .collect();

        if prices.len() < 2 {
            return 0.0;
        }

        // Calculate log returns
        let mut returns = Vec::with_capacity(prices.len() - 1);
        for i in 1..prices.len() {
            let log_return = (prices[i] / prices[i - 1]).ln();
            returns.push(log_return);
        }

        // Calculate mean
        let mean = returns.iter().sum::<f64>() / returns.len() as f64;

        // Calculate variance
        let variance = returns.iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>() / returns.len() as f64;

        // Return standard deviation
        variance.sqrt()
    }

    pub fn update(&mut self, price: f64, _timestamp: u64) {
        self.price_buffer.push(price);

        // Calculate new volatility estimate
        let sample_vol = self.calculate_std_dev();

        // EWMA smoothing
        if self.current_vol == 0.0 {
            self.current_vol = sample_vol;
        } else {
            self.current_vol = self.ewma_alpha * sample_vol
                + (1.0 - self.ewma_alpha) * self.current_vol;
        }
    }

    pub fn current_volatility(&self) -> f64 {
        // Return annualized volatility
        // Assuming quotes every 200ms = 5 per second = 18000 per hour
        let quotes_per_hour = 18000.0;
        let quotes_per_year = quotes_per_hour * 24.0 * 365.0;

        self.current_vol * quotes_per_year.sqrt()
    }
}
```

---

## 6. Lighter SDK Integration

### 6.1 Reuse Existing SDK Components

```rust
use lighter_client::{LighterClient, SignerClient};
use lighter_client::models::*;

// Use existing WebSocket connections
let (mut ws_rx, mut ws_tx) = client.connect_orderbook_stream(market_id).await?;

// Use existing order signing
let signed_tx = signer.sign_create_order(
    market_id,
    "buy",
    &format!("{:.8}", size),
    &format!("{:.8}", price),
).await?;

// Submit via WebSocket (already implemented in SDK)
let response = client.submit_transaction(&signed_tx).await?;
```

### 6.2 Market Data Streaming

```rust
async fn run_market_data_loop(
    &mut self,
    mut rx: tokio::sync::mpsc::Receiver<WsEvent>
) -> Result<(), Box<dyn std::error::Error>> {
    while let Some(event) = rx.recv().await {
        match event {
            WsEvent::Orderbook(ob) => {
                // Update market state
                if let (Some(best_bid), Some(best_ask)) =
                    (ob.bids.first(), ob.asks.first())
                {
                    self.market_state = MarketState {
                        mid_price: (best_bid.price + best_ask.price) / 2.0,
                        best_bid: best_bid.price,
                        best_ask: best_ask.price,
                        spread_bps: ((best_ask.price - best_bid.price)
                            / self.market_state.mid_price * 10000.0),
                        timestamp: chrono::Utc::now().timestamp() as u64,
                    };

                    // Trigger quote update
                    self.update_quotes().await?;
                }
            },
            WsEvent::Fill(fill) => {
                // Update inventory on fills
                self.handle_fill(fill).await?;
            },
            _ => {}
        }
    }
    Ok(())
}

async fn handle_fill(&mut self, fill: FillEvent) -> Result<(), Box<dyn std::error::Error>> {
    // Fetch updated balances from API
    let account_info = self.execution.client.get_account_info().await?;

    // Update inventory tracker
    self.inventory.update_balances(
        account_info.base_balance,
        account_info.quote_balance,
    );

    // Log fill
    tracing::info!(
        "FILL: {} {} @ {} | inventory: {:.4} -> q: {:.4}",
        fill.side,
        fill.size,
        fill.price,
        account_info.base_balance,
        self.inventory.normalized_inventory(self.market_state.mid_price),
    );

    Ok(())
}
```

---

## 7. Configuration System

### 7.1 config.toml Structure

```toml
[lighter]
api_url = "https://mainnet.zklighter.elliot.ai"
ws_url = "wss://mainnet.zklighter.elliot.ai/stream"
private_key = "${LIGHTER_PRIVATE_KEY}"

[avellaneda]
market_id = 1  # BTC-USDC
order_size = 0.001  # 0.001 BTC per side

# Avellaneda-Stoikov Parameters
gamma = 0.1           # Risk aversion (0.01 = low, 1.0 = high)
kappa = 1.5           # Liquidity parameter (higher = more liquid)
time_horizon_hours = 1.0  # Strategy horizon (1 hour)
target_base_pct = 0.5     # Target inventory (0.5 = 50% in BTC)

# Volatility Estimation
vol_lookback = 100        # Number of price samples
vol_ewma_alpha = 0.1      # EWMA smoothing (0.1 = slow, 0.5 = fast)

# Execution
refresh_interval_ms = 200  # Quote refresh rate (200ms = 5 Hz)
min_spread_bps = 5.0       # Minimum spread safety (5 bps)
max_spread_bps = 100.0     # Maximum spread safety (100 bps = 1%)

# Risk Controls
max_position = 0.01        # Maximum position in BTC
min_notional = 10.0        # Minimum order value in USDC
```

### 7.2 Config Loader

```rust
use serde::Deserialize;
use std::fs;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub lighter: LighterConfig,
    pub avellaneda: AvellanedaConfig,
}

#[derive(Debug, Deserialize)]
pub struct LighterConfig {
    pub api_url: String,
    pub ws_url: String,
    pub private_key: String,
}

impl Config {
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let config_str = fs::read_to_string("config.toml")?;

        // Expand environment variables
        let expanded = shellexpand::env(&config_str)?;

        let config: Config = toml::from_str(&expanded)?;
        Ok(config)
    }
}
```

---

## 8. Implementation Phases

### Phase 1: Core Formula Implementation (Days 1-2)

**Goal**: Implement A-S formulas and inventory tracking

**Tasks**:
1. Create module structure:
   - `types.rs` - data structures
   - `inventory.rs` - normalized inventory
   - `volatility.rs` - volatility estimator
   - `spreads.rs` - A-S formulas

2. Implement and test formulas in isolation:
   ```bash
   cargo test test_reservation_price
   cargo test test_optimal_spread
   cargo test test_normalized_inventory
   cargo test test_volatility_calculation
   ```

3. Create unit tests with known values from Hummingbot

**Deliverable**: Working formula modules with 100% test coverage

**Success Criteria**:
- Normalized inventory calculation matches Hummingbot
- Volatility estimates within 5% of expected values
- Reservation price and spread formulas produce correct outputs

### Phase 2: Market Data Integration (Days 3-4)

**Goal**: Connect to Lighter SDK and stream market data

**Tasks**:
1. Create `mm_avellaneda.rs` skeleton:
   - Initialize SDK clients
   - Connect WebSocket streams
   - Parse orderbook updates

2. Implement market state tracking:
   - Best bid/ask tracking
   - Mid price calculation
   - Timestamp management

3. Add volatility updates on price changes

4. Test with live data (read-only mode):
   ```bash
   AVELLANEDA_DRY_RUN=1 cargo run --example mm_avellaneda
   ```

**Deliverable**: Bot that streams data and calculates quotes (but doesn't place orders)

**Success Criteria**:
- Successfully connects to Lighter WebSocket
- Prints reservation price and optimal spread every 200ms
- Volatility estimate converges within 30 seconds

### Phase 3: Order Execution (Days 5-6)

**Goal**: Place and manage orders on exchange

**Tasks**:
1. Implement `execution.rs`:
   - Order placement via WebSocket
   - Order cancellation
   - Quote refresh logic

2. Add tick size snapping:
   - Fetch market info from API
   - Snap prices to valid ticks
   - Validate min notional

3. Implement cancel-replace pattern:
   ```rust
   async fn refresh_quotes(&mut self, bid: f64, ask: f64) {
       // Cancel existing orders
       if self.current_bid_id.is_some() || self.current_ask_id.is_some() {
           self.cancel_all().await?;
       }

       // Place new orders
       let bid_id = self.place_order("buy", bid, self.size).await?;
       let ask_id = self.place_order("sell", ask, self.size).await?;

       self.current_bid_id = Some(bid_id);
       self.current_ask_id = Some(ask_id);
   }
   ```

4. Test with small position:
   ```bash
   MAX_POSITION=0.0001 cargo run --example mm_avellaneda
   ```

**Deliverable**: Fully functional bot placing orders on exchange

**Success Criteria**:
- Successfully places bid and ask orders
- Cancels and replaces quotes every 200ms
- Orders appear on public orderbook

### Phase 4: Inventory Management (Days 7-8)

**Goal**: Handle fills and update inventory tracking

**Tasks**:
1. Add fill event handling:
   - Parse fill messages from WebSocket
   - Update inventory tracker
   - Recalculate normalized q

2. Test inventory skew:
   - Force fills on one side
   - Verify reservation price shifts correctly
   - Confirm inventory mean reversion

3. Add position limits:
   ```rust
   fn should_place_order(&self, side: &str, current_pos: f64) -> bool {
       match side {
           "buy" if current_pos >= self.config.max_position => false,
           "sell" if current_pos <= -self.config.max_position => false,
           _ => true,
       }
   }
   ```

**Deliverable**: Bot with working inventory management

**Success Criteria**:
- Normalized inventory calculation updates on fills
- Reservation price skews quotes toward mean reversion
- Position limits prevent excessive inventory

### Phase 5: Risk Controls (Days 9-10)

**Goal**: Add safety mechanisms and circuit breakers

**Tasks**:
1. Add spread safety bounds:
   - Min spread (prevent crossing)
   - Max spread (prevent wide quotes)

2. Add position checks:
   - Stop quoting at max position
   - Emergency cancel all

3. Add volatility circuit breakers:
   - Pause if volatility spikes >3x
   - Resume after cooldown

4. Add connection health checks:
   - Detect WebSocket disconnect
   - Auto-reconnect with backoff

**Deliverable**: Production-ready risk controls

**Success Criteria**:
- Bot survives network disconnects
- Stops quoting when position limits hit
- Pauses during extreme volatility

### Phase 6: Configuration & Tuning (Days 11-12)

**Goal**: Make bot configurable and easy to tune

**Tasks**:
1. Implement TOML config loader
2. Add parameter validation
3. Create parameter tuning guide (docs/PARAMETERS.md)
4. Add live parameter adjustment (optional)

**Deliverable**: Flexible configuration system

**Success Criteria**:
- All parameters configurable via TOML
- Invalid configs rejected with clear errors
- Documentation explains parameter impact

### Phase 7: Testing & Validation (Days 13-14)

**Goal**: Comprehensive testing before production

**Tasks**:
1. Unit tests for all modules
2. Integration tests with mock exchange
3. Backtesting (if data available)
4. Live testing with $10 position limit

**Test Matrix**:
| Scenario | Expected Behavior |
|----------|-------------------|
| Flat inventory (q=0) | Symmetric quotes around mid |
| Long inventory (q>0) | Reservation price below mid, wider bid |
| Short inventory (q<0) | Reservation price above mid, wider ask |
| High volatility | Wider spreads |
| Low volatility | Tighter spreads |
| Near max position | Stop quoting on one side |

**Deliverable**: Tested and validated bot

**Success Criteria**:
- All unit tests pass
- Bot survives 24h dry run without crashes
- P&L positive in live testing with small size

---

## 9. Testing Strategy

### 9.1 Unit Tests

Create `tests/avellaneda_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reservation_price_long_inventory() {
        let as_calc = AvellanedaStoikov::new(0.1, 1.5, 3600.0);

        let mid = 100000.0;  // $100k BTC
        let q = 0.5;         // 50% long
        let vol = 0.5;       // 50% annual vol
        let time_left = 1.0; // Full horizon remaining

        let r = as_calc.reservation_price(mid, q, vol, time_left);

        // Reservation price should be below mid when long
        assert!(r < mid, "Reservation price should be below mid for long inventory");

        // Should be approximately mid - q*gamma*vol*T
        let expected = mid - q * 0.1 * 0.5 * 1.0;
        assert!((r - expected).abs() / expected < 0.01, "Reservation price formula incorrect");
    }

    #[test]
    fn test_optimal_spread_high_risk_aversion() {
        let as_high_risk = AvellanedaStoikov::new(1.0, 1.5, 3600.0);  // High gamma
        let as_low_risk = AvellanedaStoikov::new(0.01, 1.5, 3600.0);  // Low gamma

        let vol = 0.5;
        let time_left = 1.0;

        let spread_high = as_high_risk.optimal_spread(vol, time_left);
        let spread_low = as_low_risk.optimal_spread(vol, time_left);

        assert!(spread_high > spread_low, "Higher risk aversion should produce wider spread");
    }

    #[test]
    fn test_normalized_inventory() {
        let mut tracker = InventoryTracker::new(0.5);  // 50% target

        // Portfolio: 1 BTC @ $100k + $50k = $150k total
        // Target: 50% in BTC = $75k = 0.75 BTC
        // Actual: 1 BTC
        // q = (1 - 0.75) / 1.5 = 0.25 / 1.5 = 0.167

        tracker.update_balances(1.0, 50000.0);
        let q = tracker.normalized_inventory(100000.0);

        assert!((q - 0.167).abs() < 0.01, "Normalized inventory calculation incorrect");
    }

    #[test]
    fn test_volatility_calculation() {
        let mut vol_est = VolatilityEstimator::new(10, 0.1);

        // Add prices with known 1% volatility
        let base_price = 100000.0;
        for i in 0..100 {
            let price = base_price * (1.0 + 0.01 * (i as f64 / 10.0).sin());
            vol_est.update(price, i);
        }

        let vol = vol_est.current_volatility();
        assert!(vol > 0.0, "Volatility should be positive");
        assert!(vol < 1.0, "Volatility should be reasonable");
    }
}
```

### 9.2 Integration Tests

```bash
# Test 1: Dry run mode (no real orders)
AVELLANEDA_DRY_RUN=1 cargo run --example mm_avellaneda

# Test 2: Small position live test
MAX_POSITION=0.0001 ORDER_SIZE=0.00005 cargo run --example mm_avellaneda

# Test 3: High volatility scenario
GAMMA=1.0 cargo run --example mm_avellaneda

# Test 4: Long time horizon (conservative)
TIME_HORIZON_HOURS=8 cargo run --example mm_avellaneda
```

### 9.3 Backtesting (Optional)

If historical data available:

```rust
pub struct Backtester {
    strategy: AvellanedaBot,
    historical_prices: Vec<(u64, f64)>,
    fills: Vec<Fill>,
}

impl Backtester {
    pub fn run(&mut self) -> BacktestResults {
        // Simulate strategy over historical data
        // Track P&L, Sharpe ratio, max drawdown
    }
}
```

---

## 10. Performance Targets

### 10.1 Latency Targets

| Metric | Target | Measurement |
|--------|--------|-------------|
| **Quote calculation** | <2ms | Time from orderbook update to quote ready |
| **Order placement** | <10ms | Time from quote ready to order on exchange |
| **Cancel-replace cycle** | <20ms | Full cancel + place cycle |
| **WebSocket latency** | <5ms p99 | Time from exchange to received |
| **Total loop time** | <50ms | Full cycle from data → decision → execution |

### 10.2 Resource Targets

| Metric | Target |
|--------|--------|
| **CPU usage** | <10% sustained |
| **Memory usage** | <50 MB |
| **Network traffic** | <100 KB/s sustained |
| **Order rate** | ~5 cancel-replace/sec (200ms refresh) |

### 10.3 Strategy Metrics

| Metric | Target |
|--------|--------|
| **Uptime** | >99.5% |
| **Fill rate** | 20-40% of quotes filled (healthy competition) |
| **Spread capture** | 40-60% of quoted spread |
| **Inventory drift** | Stay within ±20% of target |
| **Max drawdown** | <2% per day |

---

## 11. Migration from mm_hawkes.rs

### 11.1 What to Reuse

**Keep from mm_hawkes.rs**:
1. ✅ WebSocket connection management
2. ✅ Order signing and submission
3. ✅ Tick size snapping logic
4. ✅ Position tracking from API
5. ✅ Logging infrastructure
6. ✅ Config file loading

**Replace from mm_hawkes.rs**:
1. ❌ Hawkes intensity tracking → A-S formulas
2. ❌ Same-side refill throttling → Normalized inventory skew
3. ❌ FlipGate oscillation detector → Spread bounds
4. ❌ Crash detector → Volatility circuit breaker
5. ❌ 2-second bar volatility → Rolling window EWMA

### 11.2 Code Reuse Map

```rust
// REUSE: WebSocket setup
let (ws_rx, ws_tx) = client.connect_orderbook_stream(market_id).await?;

// REUSE: Order signing
let signed_tx = signer.sign_create_order(...)?;

// REUSE: Tick snapping
fn snap_to_tick(price: f64, tick_size: f64) -> f64 {
    (price / tick_size).round() * tick_size
}

// REPLACE: Quote calculation
// Old:
let bid = mid - spread/2 - inventory_skew - hawkes_adjustment;
// New:
let r = mid - q * gamma * vol * time_left;
let delta = gamma * vol * time_left + (2.0/gamma) * (1.0 + gamma/kappa).ln();
let bid = r - delta / 2.0;

// REPLACE: Position tracking
// Old: Absolute position + same-side throttle
// New: Normalized inventory (q = (actual - target) / total)
```

### 11.3 Simplified Architecture

**mm_hawkes.rs complexity**: ~1500 lines with many subsystems
**mm_avellaneda.rs target**: ~600-800 lines, cleaner logic

**Removed complexity**:
- No Hawkes intensity tracking
- No FlipGate oscillation detector
- No crash detector with R60/R10 thresholds
- No same-side refill cooldowns

**Simpler flow**:
```
Orderbook Update
    ↓
Calculate Volatility (rolling std dev)
    ↓
Calculate Normalized Inventory (q)
    ↓
Calculate Reservation Price (r)
    ↓
Calculate Optimal Spread (δ)
    ↓
Generate Quotes (r ± δ/2)
    ↓
Cancel-Replace Orders
```

---

## 12. Risk Controls

### 12.1 Pre-Trade Checks

```rust
impl ExecutionManager {
    async fn validate_order(&self, side: &str, price: f64, size: f64) -> Result<(), String> {
        // 1. Position limit check
        let current_pos = self.get_current_position().await?;
        if side == "buy" && current_pos >= self.max_position {
            return Err("Position limit reached".to_string());
        }
        if side == "sell" && current_pos <= -self.max_position {
            return Err("Position limit reached".to_string());
        }

        // 2. Minimum notional check
        let notional = price * size;
        if notional < self.min_notional {
            return Err(format!("Notional ${} below minimum ${}", notional, self.min_notional));
        }

        // 3. Spread bounds check
        let mid = self.market_state.mid_price;
        let spread_bps = if side == "buy" {
            (mid - price) / mid * 10000.0
        } else {
            (price - mid) / mid * 10000.0
        };

        if spread_bps < self.min_spread_bps {
            return Err(format!("Spread {} bps below minimum", spread_bps));
        }
        if spread_bps > self.max_spread_bps {
            return Err(format!("Spread {} bps above maximum", spread_bps));
        }

        Ok(())
    }
}
```

### 12.2 Circuit Breakers

```rust
pub struct CircuitBreaker {
    baseline_vol: f64,
    spike_threshold: f64,  // 3.0 = 3x baseline
    cooldown_seconds: u64,
    last_trip: Option<Instant>,
}

impl CircuitBreaker {
    pub fn should_halt(&mut self, current_vol: f64) -> bool {
        // Initialize baseline if first run
        if self.baseline_vol == 0.0 {
            self.baseline_vol = current_vol;
            return false;
        }

        // Check if volatility spiked
        let spike_ratio = current_vol / self.baseline_vol;
        if spike_ratio > self.spike_threshold {
            self.last_trip = Some(Instant::now());
            return true;
        }

        // Check if still in cooldown
        if let Some(trip_time) = self.last_trip {
            if trip_time.elapsed().as_secs() < self.cooldown_seconds {
                return true;
            }
        }

        false
    }
}
```

### 12.3 Kill Switch

```rust
impl AvellanedaBot {
    pub async fn emergency_shutdown(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        tracing::error!("EMERGENCY SHUTDOWN TRIGGERED");

        // 1. Cancel all orders
        self.execution.cancel_all().await?;

        // 2. Close position (optional - comment out if you want to keep position)
        // let position = self.get_current_position().await?;
        // if position.abs() > 0.0001 {
        //     self.close_position(position).await?;
        // }

        // 3. Stop strategy loop
        self.should_run = false;

        tracing::error!("Emergency shutdown complete");
        Ok(())
    }
}
```

---

## 13. Deployment Plan

### 13.1 Pre-Production Checklist

- [ ] All unit tests passing
- [ ] Integration tests passing
- [ ] 24h dry run successful
- [ ] Config validated
- [ ] Parameters tuned for market conditions
- [ ] Backup private key secured
- [ ] Monitoring dashboard ready
- [ ] Kill switch tested

### 13.2 Deployment Phases

**Phase 1: Canary Deployment** (Days 1-3)
- Run with $10 position limit
- Monitor for 8 hours
- Check: no crashes, reasonable P&L, inventory stays near target

**Phase 2: Limited Production** (Days 4-7)
- Increase to $50 position limit
- Run 24/7 with monitoring
- Check: uptime >99%, fill rates healthy

**Phase 3: Full Production** (Days 8+)
- Increase to target position limit
- Monitor daily P&L
- Tune parameters as needed

### 13.3 Monitoring Commands

```bash
# Watch quote updates
cargo run --example mm_avellaneda 2>&1 | grep "QUOTE"

# Monitor fills
cargo run --example mm_avellaneda 2>&1 | grep "FILL"

# Check inventory
cargo run --example mm_avellaneda 2>&1 | grep "q="

# Watch P&L (implement in bot)
cargo run --example mm_avellaneda 2>&1 | grep "PNL"
```

### 13.4 Logging Format

```rust
tracing::info!(
    "QUOTE: mid={:.2} r={:.2} δ={:.4} | bid={:.2} ask={:.2} | q={:.4} σ={:.4} t={:.3}",
    self.market_state.mid_price,
    self.strategy_state.reservation_price,
    self.strategy_state.optimal_spread,
    self.strategy_state.bid_quote,
    self.strategy_state.ask_quote,
    self.strategy_state.normalized_inventory,
    self.strategy_state.current_volatility,
    self.strategy_state.time_left_fraction,
);
```

Example output:
```
[2025-11-05 20:00:00] QUOTE: mid=102400.00 r=102398.50 δ=16.00 | bid=102390.50 ask=102406.50 | q=0.15 σ=0.45 t=0.87
[2025-11-05 20:00:00] FILL: buy 0.001 @ 102390.50 | inventory: 0.0045 -> q: 0.12
[2025-11-05 20:00:10] QUOTE: mid=102405.00 r=102403.20 δ=16.20 | bid=102395.10 ask=102411.30 | q=0.12 σ=0.46 t=0.87
```

---

## 14. Next Steps

### Immediate Actions

1. **Review Plan**: Ensure you agree with architecture and approach
2. **Setup Project**: Create initial file structure
3. **Start Phase 1**: Implement core formulas (inventory, volatility, spreads)

### Commands to Run

```bash
# 1. Add dependencies to Cargo.toml
cargo add statrs ringbuffer

# 2. Create directory structure
mkdir -p examples/trading/avellaneda
mkdir -p tests
mkdir -p docs

# 3. Create initial files
touch examples/trading/avellaneda/mm_avellaneda.rs
touch examples/trading/avellaneda/config.rs
touch examples/trading/avellaneda/inventory.rs
touch examples/trading/avellaneda/volatility.rs
touch examples/trading/avellaneda/spreads.rs
touch examples/trading/avellaneda/execution.rs
touch examples/trading/avellaneda/types.rs
touch tests/avellaneda_tests.rs

# 4. Start with unit tests
cargo test --test avellaneda_tests
```

### Questions to Address

1. **Time Horizon**: Start with 1 hour? Or shorter for faster testing?
2. **Refresh Rate**: 200ms (5 Hz) or faster like mm_hawkes 20ms?
3. **Risk Aversion**: Start conservative (γ=1.0) or moderate (γ=0.1)?
4. **Position Limits**: What's comfortable max position size?
5. **Target Inventory**: 50/50 or different allocation?

---

## 15. Comparison: Avellaneda vs mm_hawkes

| Feature | mm_hawkes.rs | mm_avellaneda.rs |
|---------|--------------|------------------|
| **Lines of Code** | ~1500 | ~800 (target) |
| **Pricing Model** | Hawkes + toxicity | Avellaneda-Stoikov |
| **Inventory** | Same-side throttle | Normalized q |
| **Volatility** | 2s bar EWMA | Rolling window EWMA |
| **Defenses** | FlipGate, crash detector | Spread bounds, circuit breaker |
| **Complexity** | High | Medium |
| **Tunability** | Many parameters | 5 core parameters |
| **Theory** | Empirical + heuristics | Pure mathematical |
| **Best For** | Adversarial markets | Efficient markets |

---

## 16. Parameter Tuning Guide

### 16.1 Risk Aversion (γ)

**Impact**: Controls how aggressively you quote

- **γ = 0.01** (Very aggressive)
  - Very tight spreads
  - High fill rate
  - High inventory risk
  - Best for: High liquidity, low volatility

- **γ = 0.1** (Moderate)
  - Balanced spreads
  - Medium fill rate
  - Controlled inventory
  - Best for: Most conditions (recommended starting point)

- **γ = 1.0** (Conservative)
  - Wide spreads
  - Low fill rate
  - Very tight inventory control
  - Best for: High volatility, uncertain markets

**Formula impact**:
```
Reservation price shift: Δr = -q × γ × σ × T
Spread component 1: γ × σ × T

Example: q=0.1, σ=0.5, T=1.0
  γ=0.01 → Δr=-0.0005, spread≈0.005  (5 bps)
  γ=0.1  → Δr=-0.05,   spread≈0.05   (50 bps)
  γ=1.0  → Δr=-0.5,    spread≈0.5    (500 bps)
```

### 16.2 Liquidity Parameter (κ)

**Impact**: Models arrival rate of market orders

- **κ = 0.5** (Low liquidity)
  - Wider spreads (less competition)
  - Longer time to fill
  - Higher profit per fill

- **κ = 1.5** (Medium liquidity - recommended)
  - Balanced spreads
  - Reasonable fill rate

- **κ = 5.0** (High liquidity)
  - Tighter spreads (high competition)
  - Fast fills
  - Lower profit per fill

**Formula impact**:
```
Spread component 2: (2/γ) × ln(1 + γ/κ)

Example: γ=0.1
  κ=0.5 → (2/0.1) × ln(1.2) = 3.65
  κ=1.5 → (2/0.1) × ln(1.067) = 1.29
  κ=5.0 → (2/0.1) × ln(1.02) = 0.40
```

### 16.3 Time Horizon (T)

**Impact**: Strategy planning window

- **T = 0.5 hour** (30 min)
  - More reactive to inventory
  - Tighter spreads
  - Faster mean reversion

- **T = 1.0 hour** (recommended)
  - Balanced approach

- **T = 4.0 hours**
  - Less reactive
  - Wider spreads
  - Slower mean reversion

**Formula impact**:
```
Both components scale with T:
  r = mid - q × γ × σ × T
  δ = γ × σ × T + liquidity_term

Short horizon → quotes more aggressive
Long horizon → quotes more conservative
```

### 16.4 Target Inventory (target_base_pct)

**Impact**: Where inventory tries to converge

- **0.3** (30% in base)
  - Bias toward quote currency
  - Reservation price shifted down
  - More selling pressure

- **0.5** (50% balanced - recommended)
  - Neutral inventory target
  - Symmetric mean reversion

- **0.7** (70% in base)
  - Bias toward base currency
  - Reservation price shifted up
  - More buying pressure

---

## 17. Expected Performance

### 17.1 Profitability Drivers

**Revenue Sources**:
1. **Spread capture**: Earn bid-ask spread on fills (30-60% capture typical)
2. **Maker rebates**: Lighter offers rebates for passive orders (check current rates)
3. **Inventory appreciation**: If market trends favorably

**Cost Factors**:
1. **Adverse selection**: Getting filled when price moves against you
2. **Inventory risk**: Holding position during adverse moves
3. **Gas fees**: Transaction costs (typically minimal on L2)

**Expected Returns**:
- **Conservative** (γ=1.0, wide spreads): 0.5-1% daily, low volatility
- **Moderate** (γ=0.1): 1-2% daily, medium volatility
- **Aggressive** (γ=0.01): 2-4% daily, high risk

### 17.2 Risk Metrics

**Sharpe Ratio Target**: >1.5 (daily)
**Max Drawdown**: <5% per week
**Win Rate**: 55-65% of days profitable
**Inventory Control**: Stay within ±30% of target

---

## Conclusion

This plan provides a complete roadmap for implementing Hummingbot's Avellaneda-Stoikov strategy in pure Rust using the Lighter SDK.

**Key Advantages**:
- ✅ Mathematical foundation (A-S 2008 paper)
- ✅ Production-proven (Hummingbot's implementation)
- ✅ Cleaner than mm_hawkes (fewer special cases)
- ✅ Tunable (5 core parameters vs dozens)
- ✅ Fast (pure Rust, no Python FFI)

**Timeline**: 2-3 weeks from start to production-ready

**First Step**: Review this plan, then begin Phase 1 (formula implementation)

---

**Created**: 2025-11-05
**Author**: Implementation plan based on Hummingbot's Avellaneda strategy
**Target**: Lighter DEX (ZKsync Era)
**Language**: 100% Rust
