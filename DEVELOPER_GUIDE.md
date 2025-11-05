# Lighter Rust SDK – Developer Guide

**Version:** 0.1.0
**Authors:** Arixon
**Last Updated:** October 2025

---

## Table of Contents

1. [Introduction](#1-introduction)
2. [Getting Started](#2-getting-started)
3. [Project Structure Overview](#3-project-structure-overview)
4. [Using the SDK in Your Own Project](#4-using-the-sdk-in-your-own-project)
5. [Environment & Configuration](#5-environment--configuration)
6. [Examples as Recipes](#6-examples-as-recipes)
7. [Testing & Validation](#7-testing--validation)
8. [Signer & Transaction Tips](#8-signer--transaction-tips)
9. [Deployment Notes](#9-deployment-notes)
10. [Appendix](#10-appendix)

---

## 1. Introduction

### 1.1 Purpose of the SDK

The **Lighter Rust SDK** is a typed, production-ready client library for interacting with the [Lighter DEX](https://lighter.xyz/) exchange platform. It provides:

- **REST API Integration**: Complete coverage of Lighter's REST endpoints for order management, account queries, market data, and position tracking
- **WebSocket Streaming**: Real-time market data, order book updates, trade fills, and position monitoring
- **Transaction Signing**: Built-in cryptographic signing for orders, cancellations, and batch transactions
- **Type Safety**: Strongly-typed Rust interfaces that prevent common API integration errors
- **Production Features**: Automatic nonce management, reconnection logic, authentication token handling, and configurable rate limiting

### 1.2 Key Design Differences vs. Official SDKs

This SDK introduces several developer-friendly patterns not found in the official TypeScript/Python SDKs:

#### ExampleContext Pattern
Rather than scattering configuration logic across examples, this SDK provides a **centralized configuration helper** (`ExampleContext`) that:
- Loads environment variables with fallback hierarchy
- Reads optional `config.toml` for per-example defaults
- Constructs pre-authenticated `LighterClient` and `SignerClient` instances
- Provides reusable WebSocket connection helpers

**Before (manual approach):**
```rust
// Every example had to repeat this boilerplate
let api_url = std::env::var("LIGHTER_API_URL")?;
let private_key = std::env::var("LIGHTER_PRIVATE_KEY")?;
let account_index = std::env::var("ACCOUNT_INDEX")?.parse::<i64>()?;
let client = LighterClient::builder()
    .api_url(api_url)
    .private_key(private_key)
    .account_index(AccountId::new(account_index))
    .build().await?;
```

**After (with ExampleContext):**
```rust
use common::ExampleContext;

let ctx = ExampleContext::initialise(Some("my_example")).await?;
let client = ctx.client();
let signer = ctx.signer()?;
let market = ctx.market_id(); // Can be overridden in config.toml
```

#### No Go FFI Required
The official SDKs delegate transaction signing to a separate Go library loaded via FFI. This SDK uses **pure Rust cryptography** (via [alloy](https://github.com/alloy-rs/alloy)) for:
- Ethereum-compatible private key derivation
- EIP-712 structured data signing
- Authentication token generation

This eliminates:
- Cross-language debugging complexity
- Separate Go library installation/compilation
- Platform-specific FFI compatibility issues

#### Config-Driven Examples
Examples support hierarchical configuration:
1. Environment variables (`.env`)
2. Example-specific config sections in `config.toml`
3. Sensible defaults

**Example `config.toml`:**
```toml
[defaults]
api_url = "https://mainnet.zklighter.elliot.ai"
market_id = 0  # ETH-PERP

[inventory_skew_adjuster]
market_id = 1  # Override to BTC-PERP for this example
target_spread_pct = 0.01
order_size = 20
```

### 1.3 Who Should Use This SDK

**Ideal for:**
- Algorithmic trading bot developers (market makers, arbitrageurs, liquidation bots)
- Portfolio management systems requiring Lighter integration
- Analytics platforms needing real-time market data
- Trading terminal developers building custom UIs

**Not ideal for:**
- Web browser applications (use TypeScript SDK instead)
- One-off scripts (REST API curl commands may be simpler)

---

## 2. Getting Started

### 2.1 Installation

Add the SDK to your `Cargo.toml`:

```toml
[dependencies]
lighter_client = { git = "https://github.com/lighter-xyz/lighter-client-rust.git", tag = "v0.1.0" }
tokio = { version = "1", features = ["full"] }
anyhow = "1.0"
```

**For examples with advanced features:**
```toml
[dev-dependencies]
dotenvy = "0.15"  # Load .env files
toml = "0.8"       # Parse config.toml
time = "0.3"       # Time utilities for order expiry
tracing-subscriber = "0.3"  # Logging
```

### 2.2 Minimum Rust Version & Prerequisites

- **Rust:** 1.70.0 or later (uses `async fn` in traits via `async-trait`)
- **OpenSSL:** Required for TLS (install via `apt install libssl-dev` on Ubuntu)
- **Network:** Outbound HTTPS (443) and WSS (443) access to Lighter mainnet

**Platform Support:**
- Linux (x86_64, aarch64)
- macOS (Intel, Apple Silicon)
- Windows (via WSL2 recommended, native support experimental)

### 2.3 Environment Setup

#### Step 1: Create `.env` File

In your project root:

```bash
# Required: Your Lighter wallet credentials
LIGHTER_PRIVATE_KEY=0x1234567890abcdef...

# Required: Account and API key indices (usually 0 for first account)
LIGHTER_ACCOUNT_INDEX=0
LIGHTER_API_KEY_INDEX=0

# Optional: Override default mainnet URL
LIGHTER_API_URL=https://mainnet.zklighter.elliot.ai

# Optional: WebSocket URL (defaults to wss://<api_domain>/stream)
LIGHTER_WS_URL=wss://mainnet.zklighter.elliot.ai/stream

# Optional: Default market (0=ETH-PERP, 1=BTC-PERP, etc.)
LIGHTER_MARKET_ID=0
```

**Security Warning:** Never commit `.env` to version control. Add to `.gitignore`:
```gitignore
.env
.env.*
!.env.example
```

#### Step 2: Create `config.toml` (Optional)

For advanced users who want per-example overrides:

```toml
# Global defaults for all examples
[defaults]
api_url = "https://mainnet.zklighter.elliot.ai"
market_id = 0

# Example-specific overrides
[my_market_maker]
market_id = 1  # Use BTC-PERP instead
spread_bps = 10
order_size = 50

[my_arbitrage_bot]
market_id = 0
max_position = 100
```

#### Step 3: Test Your Configuration

Run the quickstart example:

```bash
cargo run --example 01_quickstart
```

**Expected output:**
```
Account 281474976710656 has 0 open position(s) and 42 total order(s)
```

If you see errors:
- `LIGHTER_PRIVATE_KEY must be set` → Check your `.env` file exists and is loaded
- `error sending request for url` → Network/firewall issue or invalid `LIGHTER_API_URL`
- `invalid signature` → Private key doesn't match the account index

---

3.5 Price & Lot Decimals

Avoid hardcoding tick scaling. Use the exchange metadata or ExampleContext whenever possible to determine:
- price_decimals (number of fractional digits for price)
- size_decimals (fractional digits for contract size)

Existing examples assume ETH (price_decimals = 2, size_decimals = 4) and BTC (price_decimals = 1, size_decimals = 5). If you switch markets, adjust your integer conversions accordingly or fetch the values from the REST API (`/marketInfo`).

## 3. Project Structure Overview

```
lighter-client-rust/
├── src/                          # SDK core implementation
│   ├── lib.rs                    # Public API exports
│   ├── lighter_client/           # REST API client
│   │   └── mod.rs                # LighterClient implementation
│   ├── signer_client.rs          # Transaction signing (pure Rust)
│   ├── tx_executor.rs            # WebSocket transaction submission
│   ├── ws_client.rs              # WebSocket streaming client
│   ├── nonce_manager.rs          # Automatic nonce tracking
│   ├── types.rs                  # Type definitions (OrderId, MarketId, etc.)
│   ├── apis/                     # Generated REST API methods
│   └── models/                   # Data models (Order, Position, Trade, etc.)
│
├── examples/                     # Organized example code
│   ├── common/                   # Shared helpers for examples
│   │   ├── example_context.rs    # ExampleContext pattern
│   │   ├── env.rs                # Environment variable helpers
│   │   └── config.rs             # config.toml parser
│   │
│   ├── quickstart/               # 10 examples: basics to market making
│   │   ├── 01_quickstart.rs
│   │   ├── 02_order_lifecycle.rs
│   │   ├── 03_market_data.rs
│   │   ├── 04_account_history.rs
│   │   ├── 05_websocket.rs
│   │   ├── 06_websocket_all_channels.rs
│   │   ├── 07_websocket_transaction.rs
│   │   ├── 08_close_position.rs
│   │   ├── 09_batch_transactions.rs
│   │   └── 10_market_making_bot.rs
│   │
│   ├── order_management/         # Order placement, cancellation, batch ops
│   │   ├── test_simple_limit.rs
│   │   ├── test_postonly_order.rs
│   │   ├── test_market_order.rs
│   │   ├── test_cancel_single.rs
│   │   ├── test_batch_cancel.rs
│   │   └── ...
│   │
│   ├── rest_api/                 # REST endpoint examples
│   │   ├── test_auth_signing.rs
│   │   ├── test_get_active_orders.rs
│   │   ├── test_get_positions.rs
│   │   ├── test_send_safe_order.rs
│   │   └── ...
│   │
│   ├── websocket/                # WebSocket streaming examples
│   │   ├── subscribe_orderbook.rs
│   │   ├── subscribe_trades.rs
│   │   ├── subscribe_positions.rs
│   │   └── ...
│   │
│   ├── monitoring/               # Dashboards and data analysis
│   │   ├── orderbook_depth_analysis.rs
│   │   ├── monitor_active_orders.rs
│   │   ├── monitor_trade_fills.rs
│   │   └── multi_channel_monitor.rs
│   │
│   ├── trading/                  # Advanced trading strategies
│   │   ├── auto_order_manager.rs
│   │   ├── inventory_skew_adjuster.rs
│   │   ├── simple_order_refresher.rs
│   │   └── ...
│   │
│   └── benchmarks/               # Performance testing
│       ├── benchmark_order_latency.rs
│       ├── benchmark_close_methods.rs
│       └── bench_ws_vs_rest.rs
│
├── tests/                        # Integration tests (require mainnet access)
│   └── integration_test.rs
│
├── docs/                         # API documentation
│   └── README.md
│
├── Cargo.toml                    # Package manifest
├── README.md                     # Quick start guide
├── DEVELOPER_GUIDE.md            # This file
├── CLAUDE.md                     # Migration instructions
└── config.toml                   # Optional example configuration
```

### 3.1 Core Modules

#### `LighterClient` (`src/lighter_client/mod.rs`)
The main entry point for REST API operations:
- **Account management**: Fetch balances, positions, order history
- **Market data**: Order books, recent trades, market statistics
- **Order operations**: Place, cancel, modify orders (via REST)
- **Builder pattern**: Fluent configuration API

#### `SignerClient` (`src/signer_client.rs`)
Handles cryptographic operations:
- **EIP-712 signing**: For CreateOrder, CancelOrder, CancelAll transactions
- **Authentication**: Generates JWT-like auth tokens for private WebSocket channels
- **Nonce coordination**: Interfaces with `NonceManager` for transaction ordering

#### `WsClient` (`src/ws_client.rs`)
WebSocket streaming client:
- **Subscription management**: Order book, trades, account updates
- **Auto-reconnection**: Exponential backoff on connection failures
- **Typed events**: `WsEvent` enum for compile-time safety
- **Ping/pong**: Automatic keep-alive

#### `NonceManager` (`src/nonce_manager.rs`)
Prevents nonce collisions in concurrent trading:
- **Optimistic mode**: Predicts next nonce without REST call (low latency)
- **Conservative mode**: Fetches nonce from API before each transaction (safer)

---

## 4. Using the SDK in Your Own Project

### 4.1 Creating a Client

#### Basic Client (REST Only)

```rust
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = LighterClient::builder()
        .api_url("https://mainnet.zklighter.elliot.ai")
        .private_key("0x1234567890abcdef...")
        .account_index(AccountId::new(281474976710656))
        .api_key_index(ApiKeyIndex::new(0))
        .build()
        .await?;

    // Fetch account details
    let details = client.account().details().await?;
    println!("Account: {:?}", details);

    Ok(())
}
```

#### Advanced Client (REST + WebSocket + Signer)

```rust
use lighter_client::{
    lighter_client::LighterClient,
    nonce_manager::NonceManagerType,
    types::{AccountId, ApiKeyIndex},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = LighterClient::builder()
        .api_url("https://mainnet.zklighter.elliot.ai")
        .private_key("0x1234567890abcdef...")
        .account_index(AccountId::new(281474976710656))
        .api_key_index(ApiKeyIndex::new(0))
        // Enable optimistic nonce management (recommended for high-frequency trading)
        .nonce_management(NonceManagerType::Optimistic)
        // Configure WebSocket (defaults to wss://<api_domain>/stream)
        .websocket(
            "wss://mainnet.zklighter.elliot.ai".to_string(),
            "/stream".to_string()
        )
        .build()
        .await?;

    // Access signer for transaction signing
    let signer = client.signer().expect("signer not configured");

    // Access WebSocket builder
    let ws_builder = client.ws();

    Ok(())
}
```

### 4.2 REST Examples

#### Get Active Orders

```rust
use lighter_client::types::MarketId;

let market = MarketId::new(0); // ETH-PERP
let orders = client.order().active_orders(market).await?;

for order in orders.orders {
    println!(
        "Order {}: {} {} @ {} (filled: {}/{})",
        order.order_id,
        order.order_type,
        order.size,
        order.price,
        order.filled_quantity,
        order.size
    );
}
```

#### Get Positions

```rust
let positions = client.account().positions().await?;

for position in positions.positions {
    if position.position_value.parse::<f64>().unwrap_or(0.0) != 0.0 {
        println!(
            "Market {}: {} contracts (unrealized PnL: {})",
            position.market_id,
            position.size,
            position.unrealized_pnl
        );
    }
}
```

#### Submit Order via REST

```rust
use lighter_client::types::{BaseQty, Price, Expiry};
use time::Duration;

let market = MarketId::new(0);

// Place a post-only limit buy order
let submission = client
    .order(market)
    .buy()
    .qty(BaseQty::try_from(10000).unwrap()) // 1.0 ETH (4 decimals)
    .limit(Price::ticks(300000)) // $3000 (2 decimals)
    .expires_at(Expiry::from_now(Duration::minutes(15)))
    .post_only()
    .submit()
    .await?;

println!("Order submitted: {:?}", submission);
```

### 4.3 WebSocket Examples

#### Subscribe to Order Book

```rust
use lighter_client::{
    types::MarketId,
    ws_client::WsEvent,
};

let market = MarketId::new(0);

let mut stream = client
    .ws()
    .subscribe_order_book(market)
    .connect()
    .await?;

while let Ok(event) = stream.next_event().await {
    match event {
        WsEvent::OrderBook(ob) => {
            println!("Best bid: {:?}", ob.bids.first());
            println!("Best ask: {:?}", ob.asks.first());
        }
        WsEvent::Connected => println!("WebSocket connected"),
        WsEvent::Pong => println!("Keep-alive pong"),
        _ => {}
    }
}
```

#### Subscribe to Private Account Updates

```rust
use lighter_client::{
    types::AccountId,
    ws_client::WsEvent,
};

let account_id = AccountId::new(281474976710656);

// Generate auth token
let signer = client.signer().expect("signer required");
let auth_token = signer.create_auth_token_with_expiry(None)?.token;

// Connect with authentication
let mut stream = client
    .ws()
    .subscribe_account_all_orders(account_id)
    .subscribe_account_all_positions(account_id)
    .connect()
    .await?;

// Set auth token AFTER connection
stream.connection_mut().set_auth_token(auth_token);

while let Ok(event) = stream.next_event().await {
    match event {
        WsEvent::AccountOrder(order) => {
            println!("Order update: {:?}", order);
        }
        WsEvent::AccountPosition(pos) => {
            println!("Position update: {:?}", pos);
        }
        _ => {}
    }
}
```

### 4.4 Signer Usage

#### Sign and Submit Order via WebSocket

```rust
use lighter_client::{
    types::{BaseQty, Price, Expiry, MarketId},
    tx_executor::send_tx_ws,
};
use time::Duration;

let market = MarketId::new(0);
let signer = client.signer().expect("signer required");

// Sign the order transaction
let signed = client
    .order(market)
    .buy()
    .qty(BaseQty::try_from(10000).unwrap())
    .limit(Price::ticks(300000))
    .expires_at(Expiry::from_now(Duration::minutes(15)))
    .post_only()
    .sign()  // Returns SignedPayload instead of submitting
    .await?;

// Submit via WebSocket instead of REST
let mut ws_stream = client.ws().connect().await?;
let success = send_tx_ws(
    ws_stream.connection_mut(),
    signed.tx_type() as u8,
    signed.payload()
).await?;

println!("WebSocket submission: {}", success);
```

#### Cancel All Orders

```rust
let signer = client.signer().expect("signer required");

// Time-in-force: 0=GoodTillExpiry, 1=ImmediateOrCancel, 2=PostOnly
let time_in_force = 0;
let current_time = chrono::Utc::now().timestamp();

let tx_payload = signer
    .sign_cancel_all_orders(time_in_force, current_time, None, None)
    .await?;

// Submit via WebSocket
let mut ws_stream = client.ws().connect().await?;
let success = send_tx_ws(
    ws_stream.connection_mut(),
    3, // TX_TYPE_CANCEL_ALL_ORDERS
    &tx_payload
).await?;

println!("Cancel all submitted: {}", success);
```

---

## 5. Environment & Configuration

### 5.1 Supported Environment Variables

The SDK recognizes the following environment variables (in order of precedence):

| Variable | Aliases | Type | Required | Description |
|----------|---------|------|----------|-------------|
| `LIGHTER_PRIVATE_KEY` | - | hex string | **Yes** | Ethereum private key (0x-prefixed) |
| `LIGHTER_ACCOUNT_INDEX` | `ACCOUNT_INDEX`, `LIGHTER_ACCOUNT_ID` | i64 | **Yes** | Account index from Lighter API |
| `LIGHTER_API_KEY_INDEX` | `API_KEY_INDEX` | i32 | **Yes** | API key slot (usually 0) |
| `LIGHTER_API_URL` | `LIGHTER_HTTP_BASE`, `LIGHTER_URL` | URL | No | API base URL (default: mainnet) |
| `LIGHTER_WS_URL` | `LIGHTER_WS_BASE` | URL | No | WebSocket URL (auto-derived from API URL) |
| `LIGHTER_MARKET_ID` | `MARKET_ID` | i32 | No | Default market (0=ETH, 1=BTC, etc.) |

### 5.2 `examples/common/env.rs` Helpers

The SDK provides utility functions for environment parsing:

#### `require_env(keys, label)`
```rust
// Tries multiple environment variables, panics if none found
let private_key = require_env(
    &["LIGHTER_PRIVATE_KEY", "PRIVATE_KEY"],
    "private key"
);
```

#### `parse_env(keys)`
```rust
// Returns Option<T> if variable exists and parses successfully
let market_id: Option<i32> = parse_env(&["LIGHTER_MARKET_ID", "MARKET_ID"]);
```

#### `require_parse_env(keys, label)`
```rust
// Combines require_env + parse, panics on missing/invalid value
let account_index: i64 = require_parse_env(
    &["LIGHTER_ACCOUNT_INDEX", "ACCOUNT_INDEX"],
    "account index"
);
```

#### `ensure_positive(value, label)`
```rust
// Validates numeric value is positive (non-zero)
let account_index = ensure_positive(
    require_parse_env(&["ACCOUNT_INDEX"], "account index"),
    "account index"
);
```

#### `resolve_api_url(default)`
```rust
// Resolves API URL with fallback to default
let api_url = resolve_api_url("https://mainnet.zklighter.elliot.ai");
// Checks: LIGHTER_API_URL → LIGHTER_HTTP_BASE → LIGHTER_URL → default
```

### 5.3 `config.toml` Defaults & Overrides

Create a `config.toml` in your project root:

```toml
# Global defaults applied to all examples
[defaults]
api_url = "https://mainnet.zklighter.elliot.ai"
market_id = 0  # ETH-PERP

# Example-specific section: [example_name_without_extension]
[inventory_skew_adjuster]
market_id = 1  # Override to BTC-PERP for this example
target_spread_pct = 0.01
order_size = 20
max_inventory = 100

[auto_order_manager]
market_id = 0
refresh_interval_ms = 5000
spread_bps = 15
```

**How ExampleContext Uses This:**

```rust
// examples/common/example_context.rs

impl ExampleConfig {
    pub fn load(example_key: Option<&str>) -> Self {
        // Priority:
        // 1. Environment variables (highest)
        // 2. config.toml [example_key] section
        // 3. config.toml [defaults] section
        // 4. Hardcoded fallback (lowest)

        let market_id = parse_env::<i32>(&["LIGHTER_MARKET_ID", "MARKET_ID"])
            .or_else(|| {
                example_key.and_then(|key| {
                    get_i64_path(&format!("{key}.market_id")).map(|v| v as i32)
                })
            })
            .or_else(|| get_i64_path("defaults.market_id").map(|v| v as i32))
            .unwrap_or(0);

        // ... similar for other fields
    }
}
```

**Usage in Examples:**

```rust
// examples/trading/inventory_skew_adjuster.rs

#[tokio::main]
async fn main() -> Result<()> {
    // Loads config.toml [inventory_skew_adjuster] section
    let ctx = ExampleContext::initialise(Some("inventory_skew_adjuster")).await?;

    let market = ctx.market_id(); // Gets market_id from config hierarchy
    let client = ctx.client();
    let signer = ctx.signer()?;

    // ... rest of implementation
}
```

### 5.4 Best Practices

**DO:**
- Use `.env` for secrets (private keys, API keys)
- Use `config.toml` for non-sensitive defaults (URLs, market IDs, strategy parameters)
- Add `.env` to `.gitignore`
- Commit `.env.example` with dummy values for documentation

**DON'T:**
- Hardcode private keys or account indices in code
- Commit actual `.env` files
- Use `unwrap()` on environment parsing (use `require_env` helpers instead)

---

## 6. Examples as Recipes

The SDK includes 74+ runnable examples demonstrating every SDK feature. This section provides a guided tour.

### 6.1 Quickstart Walkthrough

#### **01_quickstart.rs** - Fetch Account Metadata
**Purpose:** Verify credentials and environment setup.

```rust
cargo run --example 01_quickstart
```

**What it does:**
- Loads environment variables
- Builds `LighterClient`
- Fetches account details (positions, order count)

**Source:** `examples/quickstart/01_quickstart.rs:36-42`

---

#### **02_order_lifecycle.rs** - Place and Cancel Order
**Purpose:** Complete order lifecycle (create → monitor → cancel).

```rust
cargo run --example 02_order_lifecycle
```

**What it does:**
1. Places a post-only limit order
2. Polls for order status via REST
3. Cancels the order
4. Confirms cancellation

**Key pattern:**
```rust
// Place order
let submission = client.order(market).buy()
    .qty(BaseQty::try_from(10000)?)
    .limit(Price::ticks(280000))
    .post_only()
    .submit().await?;

// Cancel order
client.cancel(market, submission.order_id).submit().await?;
```

---

#### **03_market_data.rs** - Fetch Order Book and Trades
**Purpose:** Query public market data.

```rust
cargo run --example 03_market_data
```

**What it does:**
- Fetches order book snapshot
- Fetches recent trades
- Calculates spread and depth

---

#### **05_websocket.rs** - Real-Time Order Book
**Purpose:** Stream live market data.

```rust
cargo run --example 05_websocket
```

**What it does:**
- Subscribes to order book channel
- Prints best bid/ask on each update
- Handles reconnection automatically

**Key pattern:**
```rust
let mut stream = client.ws()
    .subscribe_order_book(market)
    .connect().await?;

while let Ok(event) = stream.next_event().await {
    match event {
        WsEvent::OrderBook(ob) => {
            println!("Spread: {} bps", calculate_spread(&ob));
        }
        _ => {}
    }
}
```

---

#### **09_batch_transactions.rs** - Atomic Multi-Order Placement
**Purpose:** Submit multiple orders in a single transaction.

```rust
cargo run --example 09_batch_transactions
```

**What it does:**
- Signs 5 orders (buy and sell ladder)
- Submits them atomically via WebSocket
- All succeed or all fail (no partial fills)

**Key pattern:**
```rust
let signer = client.signer().unwrap();

// Sign multiple orders
let signed1 = client.order(market).buy().limit(...).sign().await?;
let signed2 = client.order(market).sell().limit(...).sign().await?;

// Batch submit
let payloads = vec![signed1.payload(), signed2.payload()];
send_tx_batch_ws(ws_conn, payloads).await?;
```

---

### 6.2 Order Management Recipes

#### **test_simple_limit.rs** - Basic Limit Order
Place a post-only limit order at specified price:

```bash
cargo run --example test_simple_limit
```

---

#### **test_market_order.rs** - Market Order (IOC)
Execute immediate-or-cancel market order:

```bash
cargo run --example test_market_order
```

**Key differences from limit:**
```rust
// Limit order
.limit(Price::ticks(300000))

// Market order
.market()  // No price specified
.ioc()     // Immediate-or-cancel time-in-force
```

---

#### **test_reduceonly_order.rs** - Reduce-Only Order
Only allows position reduction (prevents accidental flips):

```bash
cargo run --example test_reduceonly_order
```

**Key pattern:**
```rust
client.order(market)
    .sell()  // Close long position
    .market()
    .reduce_only()  // Cannot open short if no long exists
    .submit().await?;
```

---

#### **test_cancel_replace_batch.rs** - Atomic Cancel+Replace
Cancel existing orders and place new ones atomically:

```bash
cargo run --example test_cancel_replace_batch
```

**Use case:** Market maker order refreshing without risk of being unhedged.

---

### 6.3 REST Integration Recipes

#### **test_get_active_orders.rs**
Fetch all active orders across markets:

```bash
cargo run --example test_get_active_orders
```

---

#### **test_get_positions.rs**
Query current positions with unrealized PnL:

```bash
cargo run --example test_get_positions
```

---

#### **test_send_safe_order.rs**
Submit order with pre-flight validation:

```bash
cargo run --example test_send_safe_order
```

**Safety checks:**
- Verifies account has sufficient margin
- Checks order doesn't exceed position limits
- Validates price is within circuit breaker bounds

---

### 6.4 WebSocket Demos

#### **subscribe_orderbook.rs** - Order Book Streaming
```bash
cargo run --example subscribe_orderbook
```

Displays real-time order book with depth visualization.

---

#### **subscribe_trades.rs** - Trade Tape
```bash
cargo run --example subscribe_trades
```

Prints all trades as they occur on the market.

---

#### **06_websocket_all_channels.rs** - Multi-Channel Monitor
```bash
cargo run --example 06_websocket_all_channels
```

Subscribes to 8+ channels simultaneously:
- Order book
- Recent trades
- Market stats
- Account orders
- Account positions
- Account trade fills

---

### 6.5 Trading Utilities

#### **auto_order_manager.rs** - Auto-Refreshing Market Maker
```bash
cargo run --example auto_order_manager
```

**What it does:**
- Places bid/ask orders around mid-price
- Refreshes orders every N seconds
- Cancels and replaces stale orders
- Monitors inventory limits

**Configuration (config.toml):**
```toml
[auto_order_manager]
market_id = 0
refresh_interval_ms = 5000
spread_bps = 15
order_size = 10
max_inventory = 50
```

---

#### **inventory_skew_adjuster.rs** - Skew-Based Quoting
```bash
cargo run --example inventory_skew_adjuster
```

**What it does:**
- Adjusts bid/ask spreads based on current inventory
- Skews quotes to encourage mean reversion
- Example: If long 10 ETH, widens bids and tightens asks

**Formula:**
```rust
let skew_factor = current_position / max_position;
let bid_spread = base_spread * (1.0 + skew_factor);
let ask_spread = base_spread * (1.0 - skew_factor);
```

See `examples/INVENTORY_SKEW_README.md` for detailed explanation.

---

#### **simple_order_refresher.rs** - Minimal Market Maker
```bash
cargo run --example simple_order_refresher
```

**What it does:**
- Places single bid/ask orders
- Refreshes every 10 seconds
- No inventory management (for testing only)

---

### 6.6 Benchmarks

#### **benchmark_order_latency.rs** - Measure Order Roundtrip
```bash
cargo run --example benchmark_order_latency
```

**What it measures:**
- Order submission time (sign + submit)
- Order confirmation time (API acknowledgment)
- Order cancellation time

**Sample output:**
```
Order submission: 42ms
Order confirmation: 67ms
Order cancellation: 38ms
```

---

#### **bench_ws_vs_rest.rs** - Compare WebSocket vs REST Latency
```bash
cargo run --example bench_ws_vs_rest
```

**Typical results:**
- REST order submission: 80-120ms
- WebSocket order submission: 30-50ms
- WebSocket reduces latency by ~60%

---

## 7. Testing & Validation

### 7.1 Running Individual Examples

```bash
# Run specific example
cargo run --example 01_quickstart

# With logging enabled
RUST_LOG=debug cargo run --example 01_quickstart

# With custom timeout (for long-running examples)
timeout 60s cargo run --example auto_order_manager
```

### 7.2 "Run All Examples" Test Harness

To validate SDK changes across all examples:

```bash
cd /path/to/lighter-client-rust
source .env  # Load environment variables

# Run all examples with 10s timeout
bash -c '
set -euo pipefail
log=all_example_log.md
echo "# All Examples Test Log" >"$log"
echo "" >>"$log"
echo "**Date:** $(date)" >>"$log"
examples=( $(find examples -maxdepth 2 -type f -name "*.rs" ! -path "examples/common/*") )
echo "" >>"$log"
echo "**Total Examples:** ${#examples[@]}" >>"$log"
echo "" >>"$log"

for src in "${examples[@]}"; do
  ex_path=${src#examples/}
  ex_name=${ex_path%.rs}
  echo -e "\n---\n\n## Example: ${ex_name}\n\n\`\`\`" >>"$log"
  if timeout 10s cargo run --quiet --example "${ex_name##*/}" >>"$log" 2>&1; then
    status="✅ PASSED"
    code=0
  else
    if [ "$?" -eq 124 ]; then
      status="⏱️ TIMEOUT (10s)"
      code=124
    else
      status="❌ FAILED"
      code=1
    fi
  fi
  echo -e "\`\`\`\n\n**Status:** ${status} (exit code: ${code})\n\nSource: \`examples/${ex_path}\`\n\n---\n" >>"$log"
  echo "Completed: ${ex_name} - ${status}"
done

echo "Saved: $log"
'
```

**Expected results (as of October 2025):**
- **PASSED:** ~32 examples (43%)
- **TIMEOUT:** ~41 examples (55%) - Long-running monitors/market makers
- **FAILED:** 0 examples (0%)

**Note:** Timeouts are expected for examples like `auto_order_manager` that run indefinitely.

### 7.3 Cargo Check for Examples

Validate example code compiles without running:

```bash
# Check all examples
cargo check --examples

# Check specific example
cargo check --example 01_quickstart

# With warnings as errors
cargo check --examples -- -D warnings
```

### 7.4 Integration Tests

Run integration tests (requires mainnet access):

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_create_and_cancel_order

# With output
cargo test -- --nocapture
```

**Note:** Integration tests place real orders on mainnet. Use small sizes and cancel immediately.

---

## 8. Signer & Transaction Tips

### 8.1 Nonce Management

The SDK supports two nonce management strategies:

#### Conservative (Default)
Fetches nonce from API before each transaction:

```rust
let client = LighterClient::builder()
    .nonce_management(NonceManagerType::Conservative)
    .build().await?;
```

**Pros:** Guaranteed correctness, no nonce collisions
**Cons:** +50-100ms latency per transaction (extra REST call)

#### Optimistic (Recommended for HFT)
Predicts next nonce without API call:

```rust
let client = LighterClient::builder()
    .nonce_management(NonceManagerType::Optimistic)
    .build().await?;
```

**Pros:** Zero latency overhead
**Cons:** Rare nonce collisions if multiple clients use same account

**When to use Optimistic:**
- Single bot per account
- High-frequency trading (>10 orders/sec)
- Latency is critical

**When to use Conservative:**
- Multiple bots sharing same account
- Infrequent trading (<1 order/sec)
- Maximum reliability required

### 8.2 Batch Submissions

Submit multiple orders atomically:

```rust
use lighter_client::tx_executor::send_tx_batch_ws;

let signer = client.signer().unwrap();

// Sign orders individually
let signed1 = client.order(market).buy().limit(...).sign().await?;
let signed2 = client.order(market).sell().limit(...).sign().await?;
let signed3 = client.cancel(market, order_id).sign().await?;

// Batch submit via WebSocket
let mut ws = client.ws().connect().await?;
let payloads = vec![
    signed1.payload(),
    signed2.payload(),
    signed3.payload(),
];

let success = send_tx_batch_ws(ws.connection_mut(), payloads).await?;
```

**Benefits:**
- Atomic execution (all-or-nothing)
- Reduced network overhead
- Lower latency than sequential submissions

**Limits:**
- Maximum 50 transactions per batch (Lighter API limit)

### 8.3 Reduce-Only + Slippage Settings

Prevent accidental position flips:

```rust
// Safe position close (cannot increase position)
client.order(market)
    .sell()  // Close long
    .qty(BaseQty::try_from(current_position)?)
    .market()
    .reduce_only()  // Key: prevents flip to short
    .ioc()
    .submit().await?;
```

**Without reduce_only:**
- If you're long 10 ETH and sell 15 ETH
- Position becomes short 5 ETH

**With reduce_only:**
- If you're long 10 ETH and sell 15 ETH
- Only 10 ETH executes, position closes to 0
- Extra 5 ETH rejected

### 8.4 Order Expiry Best Practices

**Minimum expiry:** 10 minutes from now

```rust
use time::Duration;

// ✅ GOOD: 15 minute expiry
.expires_at(Expiry::from_now(Duration::minutes(15)))

// ❌ BAD: 5 minute expiry (rejected by API)
.expires_at(Expiry::from_now(Duration::minutes(5)))  // Error!

// ✅ GOOD: 7 day expiry (max)
.expires_at(Expiry::from_now(Duration::days(7)))
```

**Why 10 minutes minimum?**
- Prevents accidental expiry during network issues
- Allows time for order to rest in book
- API rejects shorter durations

**Recommended for market makers:**
```rust
// Refresh orders every 5 minutes, but set 15 minute expiry
// This gives 10 minute buffer if refresh fails
.expires_at(Expiry::from_now(Duration::minutes(15)))
```

### 8.5 Handling Errors

```rust
match client.order(market).buy().limit(...).submit().await {
    Ok(submission) => {
        println!("Order placed: {}", submission.order_id);
    }
    Err(e) => {
        // Check error type
        if e.to_string().contains("insufficient margin") {
            eprintln!("Not enough margin, reduce order size");
        } else if e.to_string().contains("invalid signature") {
            eprintln!("Check private key matches account index");
        } else if e.to_string().contains("nonce") {
            eprintln!("Nonce collision, retry with Conservative mode");
        } else {
            eprintln!("Order failed: {}", e);
        }
    }
}
```

---

## 9. Deployment Notes

### 9.1 Removing Dev Artefacts

Before deploying to production, clean up development files:

```bash
# Remove build artifacts
cargo clean

# Remove logs
rm -f *.log all_example_log.md

# Remove test databases (if any)
rm -rf test_data/

# Remove example-only dependencies from Cargo.toml
# (Keep only dependencies, remove dev-dependencies if not needed)
```

### 9.2 Packaging for Distribution

**Recommended files to include:**

```
my-lighter-bot/
├── Cargo.toml
├── Cargo.lock  # Pin dependency versions for reproducibility
├── src/
│   └── main.rs
├── .env.example  # Documentation only, no secrets
├── config.toml.example
└── README.md
```

**DO NOT include:**
- `.env` (contains secrets)
- `target/` (build artifacts)
- `*.log` files
- `.git/` (if distributing as tarball)

### 9.3 Production Checklist

**Before deploying:**

- [ ] Test on mainnet with small orders
- [ ] Set up monitoring (order fill notifications, position alerts)
- [ ] Configure systemd/supervisor for auto-restart
- [ ] Set up log rotation (`logrotate`)
- [ ] Test WebSocket reconnection (simulate network failures)
- [ ] Verify `.env` is in `.gitignore`
- [ ] Document emergency shutdown procedure
- [ ] Set position limits in code (not just config)
- [ ] Test with `NonceManagerType::Conservative` first
- [ ] Monitor nonce collisions in logs

**Recommended systemd unit (`/etc/systemd/system/lighter-bot.service`):**

```ini
[Unit]
Description=Lighter Trading Bot
After=network.target

[Service]
Type=simple
User=lighter
WorkingDirectory=/opt/lighter-bot
EnvironmentFile=/opt/lighter-bot/.env
ExecStart=/opt/lighter-bot/target/release/lighter-bot
Restart=on-failure
RestartSec=10
StandardOutput=append:/var/log/lighter-bot/stdout.log
StandardError=append:/var/log/lighter-bot/stderr.log

[Install]
WantedBy=multi-user.target
```

**Start/stop commands:**
```bash
sudo systemctl start lighter-bot
sudo systemctl status lighter-bot
sudo systemctl stop lighter-bot
sudo journalctl -u lighter-bot -f  # View logs
```

---

## 10. Appendix

### 10.1 Environment Variable Reference Table

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `LIGHTER_PRIVATE_KEY` | hex string | **required** | Ethereum private key (0x-prefixed) |
| `LIGHTER_ACCOUNT_INDEX` | i64 | **required** | Account index from Lighter API |
| `LIGHTER_API_KEY_INDEX` | i32 | **required** | API key slot (usually 0-3) |
| `LIGHTER_API_URL` | URL | `https://mainnet.zklighter.elliot.ai` | REST API base URL |
| `LIGHTER_WS_URL` | URL | Derived from API URL | WebSocket URL |
| `LIGHTER_MARKET_ID` | i32 | 0 (ETH-PERP) | Default market for examples |
| `ACCOUNT_INDEX` | i64 | - | Alias for `LIGHTER_ACCOUNT_INDEX` |
| `API_KEY_INDEX` | i32 | - | Alias for `LIGHTER_API_KEY_INDEX` |
| `RUST_LOG` | log filter | `info` | Tracing level (debug, info, warn, error) |

### 10.2 ExampleContext Configuration Flowchart

```
┌─────────────────────────────────────────────────────────────┐
│ ExampleContext::initialise(Some("my_example"))             │
└─────────────────────────┬───────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────┐
│ For each config field (market_id, api_url, etc.):          │
└─────────────────────────┬───────────────────────────────────┘
                          │
                          ▼
          ┌───────────────────────────────┐
          │ 1. Check environment variable │
          │    (e.g., LIGHTER_MARKET_ID)  │
          └───────────┬───────────────────┘
                      │
                      │ Not found
                      ▼
          ┌───────────────────────────────┐
          │ 2. Check config.toml          │
          │    [my_example] section       │
          └───────────┬───────────────────┘
                      │
                      │ Not found
                      ▼
          ┌───────────────────────────────┐
          │ 3. Check config.toml          │
          │    [defaults] section         │
          └───────────┬───────────────────┘
                      │
                      │ Not found
                      ▼
          ┌───────────────────────────────┐
          │ 4. Use hardcoded default      │
          │    (e.g., market_id = 0)      │
          └───────────────────────────────┘
```

### 10.3 Troubleshooting Common Errors

#### Error: `LIGHTER_PRIVATE_KEY must be set`
**Cause:** Environment variable not loaded.

**Fix:**
```bash
# Check .env file exists
ls -la .env

# Load manually
source .env

# Or use dotenvy in code
dotenvy::dotenv().ok();
```

---

#### Error: `invalid value: integer 4294967295, expected i32`
**Cause:** API returns u32::MAX in i32 field (order IDs, indices).

**Fix:**
This was fixed in recent SDK versions by changing affected fields to i64. Update to latest:
```bash
cargo update -p lighter_client
```

**Source:** Fixed in `src/types/orders.rs` (changed InactiveOrder fields to i64)

---

#### Error: `expiry must be at least 10 minutes from now`
**Cause:** Order expiry set too short.

**Fix:**
```rust
// ❌ BAD
.expires_at(Expiry::from_now(Duration::minutes(5)))

// ✅ GOOD
.expires_at(Expiry::from_now(Duration::minutes(15)))
```

---

#### Error: `dns error: failed to lookup address information`
**Cause:** Cannot resolve Lighter API domain.

**Fix:**
```bash
# Test DNS resolution
nslookup mainnet.zklighter.elliot.ai

# Check firewall allows outbound HTTPS
curl -v https://mainnet.zklighter.elliot.ai/v1/health

# Try alternative API URL
export LIGHTER_API_URL=https://backup.lighter.xyz
```

---

#### Error: `invalid signature`
**Cause:** Private key doesn't match account index.

**Fix:**
1. Verify account index:
   ```bash
   curl -X GET "https://mainnet.zklighter.elliot.ai/v1/accounts?address=0xYOUR_ADDRESS"
   ```

2. Check private key corresponds to address:
   ```rust
   use alloy::signers::local::PrivateKeySigner;

   let signer = PrivateKeySigner::from_str("0x...")?;
   println!("Address: {:?}", signer.address());
   ```

---

#### Error: `WebSocket closed: Some(CloseFrame { code: Normal, reason: "" })`
**Cause:** Normal closure or server-side timeout.

**Fix:**
The SDK auto-reconnects. If reconnections fail:
```rust
// Increase backoff limits
use lighter_client::ws_client::BackoffConfig;

let client = LighterClient::builder()
    .websocket_with_backoff(
        ws_host,
        ws_path,
        BackoffConfig {
            initial_delay_ms: 1000,
            max_delay_ms: 60000,
            max_retries: 10,
        }
    )
    .build().await?;
```

---

#### Error: `nonce too low` or `nonce too high`
**Cause:** Nonce collision (multiple clients or failed optimistic prediction).

**Fix:**
Switch to Conservative mode:
```rust
let client = LighterClient::builder()
    .nonce_management(NonceManagerType::Conservative)  // Fetch from API
    .build().await?;
```

For high-frequency use, ensure only ONE client per account uses Optimistic mode.

---

### 10.4 Migration Checklist (websocket_ref → lighter_client)

If you have legacy code using `websocket_ref` and manual FFI, follow this checklist:

- [ ] **Replace manual env parsing** with `ExampleContext::initialise()`
  ```rust
  // Before:
  let api_url = std::env::var("LIGHTER_API_URL")?;

  // After:
  let ctx = ExampleContext::initialise(Some("my_script")).await?;
  let client = ctx.client();
  ```

- [ ] **Replace FfiGoSigner** with `SignerClient`
  ```rust
  // Before:
  let signer = FfiGoSigner::new(...)?;
  let signed_tx = signer.sign_create_order(...)?;

  // After:
  let signer = ctx.signer()?;
  let signed = client.order(market).buy().limit(...).sign().await?;
  ```

- [ ] **Replace manual WebSocket connections** with `WsBuilder`
  ```rust
  // Before:
  let mut ws = WsClient::connect("wss://...").await?;

  // After:
  let mut stream = ctx.ws_builder()
      .subscribe_order_book(market)
      .connect().await?;
  ```

- [ ] **Replace manual nonce fetching** with SDK nonce manager
  ```rust
  // Before:
  let nonce = fetch_nonce_from_api(account_id).await?;
  let signed_tx = signer.sign_create_order(..., nonce, ...)?;

  // After:
  // SDK handles nonce automatically
  let signed = client.order(market).buy().sign().await?;
  ```

- [ ] **Replace raw JSON parsing** with typed `WsEvent`
  ```rust
  // Before:
  let msg = ws.recv().await?;
  let json: serde_json::Value = serde_json::from_str(&msg)?;
  if json["type"] == "orderbook" { ... }

  // After:
  let event = stream.next_event().await?;
  match event {
      WsEvent::OrderBook(ob) => { ... }
      _ => {}
  }
  ```

- [ ] **Remove hardcoded constants** (API URLs, market IDs)
  ```rust
  // Before:
  const MARKET_ID: i32 = 0;
  const API_URL: &str = "https://mainnet.zklighter.elliot.ai";

  // After:
  let market = ctx.market_id();  // From config or env
  let api_url = ctx.config.api_url;  // From config hierarchy
  ```

- [ ] **Run cargo check** to verify no compilation errors
  ```bash
  cargo check --example my_script
  ```

- [ ] **Test with small orders** on mainnet before full deployment

---

### 10.5 Market IDs Reference

| Market ID | Symbol | Description |
|-----------|--------|-------------|
| 0 | ETH-PERP | Ethereum perpetual futures |
| 1 | BTC-PERP | Bitcoin perpetual futures |
| 2+ | TBD | Check `/v1/markets` API for full list |

**Fetch all markets:**
```bash
curl -X GET "https://mainnet.zklighter.elliot.ai/v1/markets"
```

---

### 10.6 Additional Resources

**Official Documentation:**
- Lighter API Docs: https://apidocs.lighter.xyz/
- Lighter Main Docs: https://docs.lighter.xyz/

**Community:**
- Discord: https://discord.gg/lighter
- GitHub Issues: https://github.com/lighter-xyz/lighter-client-rust/issues

**Related Tools:**
- TypeScript SDK: https://github.com/lighter-xyz/lighter-ts-sdk
- Python SDK: https://github.com/lighter-xyz/lighter-py-sdk

---

## Conclusion

This guide covers the essential patterns for building production trading systems with the Lighter Rust SDK. For more examples and advanced use cases, explore the `examples/` directory in the repository.

**Next steps:**
1. Run the quickstart examples (`01_quickstart.rs` through `10_market_making_bot.rs`)
2. Study the example most similar to your use case
3. Copy its structure and modify for your strategy
4. Test with small orders on mainnet
5. Deploy to production with monitoring and safety limits

**Happy trading!**
