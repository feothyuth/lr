# Lighter WebSocket Channels Guide

Complete reference for all WebSocket channels available in the Lighter exchange API.

**Official Documentation:** https://apidocs.lighter.xyz/docs/websocket-reference

---

## Table of Contents

1. [Public Channels](#public-channels)
   - [Order Book](#order-book)
   - [BBO (Best Bid/Offer)](#bbo-best-bidoffer)
   - [Trade](#trade)
   - [Market Stats](#market-stats)
   - [Transaction](#transaction-deprecated)
   - [Executed Transaction](#executed-transaction-deprecated)
   - [Height](#height)
2. [Private Channels](#private-channels) (Require Authentication)
   - [Account All](#account-all)
   - [Account Market](#account-market)
   - [User Stats (Account Stats)](#user-stats-account-stats)
   - [Account Tx](#account-tx)
   - [Account All Orders](#account-all-orders)
   - [Account Orders](#account-orders)
   - [Account All Trades](#account-all-trades)
   - [Account Market Trades](#account-market-trades)
   - [Account All Positions](#account-all-positions)
   - [Pool Data](#pool-data)
   - [Pool Info](#pool-info)
   - [Notification](#notification)
3. [Connection Management](#connection-management)
4. [Performance Tips](#performance-tips)
5. [Common Patterns](#common-patterns)

---

## Public Channels

Public channels do not require authentication and provide market-wide data.

### Order Book

**Channel:** `order_book/{MARKET_INDEX}`

**Subscribe (Rust SDK):**
```rust
.subscribe_order_book(market_id)
```

**Event Type:** `WsEvent::OrderBook(OrderBookEvent)`

**Data Provided:**
- Full order book state (all price levels)
- Best bid/ask prices and sizes
- Incremental updates (deltas)
- **Nonce** (Premium feature) - Sequence number to detect missed updates

**Fields:**
```rust
OrderBookEvent {
    market: MarketId,           // Market ID
    state: OrderBookState,      // Current full book state
    delta: Option<OrderBookDelta>, // Changes since last update
    nonce: Option<u64>,         // Sequence number (Premium only)
}

OrderBookState {
    asks: Vec<OrderBookLevel>,  // Sell side
    bids: Vec<OrderBookLevel>,  // Buy side
}

OrderBookLevel {
    price: String,              // Price level
    size: String,               // Total size at this level
}
```

**Use Cases:**
- Full order book visualization
- Depth analysis
- Liquidity tracking
- Market making strategies

**Reliability:**
- Use `nonce` field to detect gaps (Premium accounts)
- Request REST snapshot when gaps detected
- Typical update frequency: 10-100ms per update

**Example:**
```rust
WsEvent::OrderBook(update) => {
    if let Some(nonce) = update.nonce {
        // Check for missed updates
        if nonce != expected_nonce + 1 {
            println!("⚠️ Gap detected! Request fresh snapshot");
        }
    }

    let best_bid = update.state.bids.first();
    let best_ask = update.state.asks.first();
}
```

---

### BBO (Best Bid/Offer)

**Subscribe (Rust SDK):**
```rust
.subscribe_bbo(market_id)
```

**Event Type:** `WsEvent::BBO(BBOEvent)`

**Data Provided:**
- Best bid price and size
- Best ask price and size
- Only sends when TOB changes

**Use Cases:**
- Lightweight price tracking
- Spread monitoring
- Simple trading strategies
- Reduced bandwidth vs full order book

**Advantages:**
- Lower message volume (only when TOB changes)
- BBO is ~80-90% fewer messages than full order book

---

### Trade

**Channel:** `trade/{MARKET_INDEX}`

**Subscribe (Rust SDK):**
```rust
.subscribe_trade(market_id)
```

**Event Type:** `WsEvent::Trade(TradeEvent)`

**Data Provided:**
- Public trades for the market
- Price, size, side
- Order IDs (ask_id, bid_id)
- Account IDs
- **NEW:** ask_client_id and bid_client_id fields

**Fields:**
```json
{
  "channel": "trade/1",
  "trades": [
    {
      "trade_id": 449491731,
      "tx_hash": "0x...",
      "type": "trade",
      "market_id": 1,
      "size": "0.001",
      "price": "105000.0",
      "usd_amount": "105.0",
      "ask_id": 562950522978024,
      "bid_id": 844424346732582,
      "ask_client_id": 264308218600602,  // NEW
      "bid_client_id": 0,                // NEW
      "ask_account_id": 257024,
      "bid_account_id": 281474976665656,
      "is_maker_ask": true,
      "block_height": 12345,
      "timestamp": 1762860622253
    }
  ]
}
```

**Use Cases:**
- Market activity monitoring
- Volume tracking
- Price discovery
- Tape reading

**Note:** This shows ALL trades on the market. For YOUR trades only, use `subscribe_account_market_trades()`.

---

### Market Stats

**Channel:** `market_stats/{MARKET_INDEX}` or `market_stats/all`

**Subscribe (Rust SDK):**
```rust
.subscribe_market_stats(market_id)
```

**Event Type:** `WsEvent::MarketStats(MarketStatsEvent)`

**Data Provided:**
- Index price (oracle price)
- Mark price (fair price for liquidations)
- Open interest
- Funding rates (current and next)
- 24h volume, high, low, price change

**Fields:**
```json
{
  "channel": "market_stats/1",
  "market_stats": {
    "market_id": 1,
    "index_price": "105000.50",
    "mark_price": "105010.25",
    "open_interest": "1250000.0",
    "last_trade_price": "105005.0",
    "current_funding_rate": "0.0001",
    "funding_rate": "0.00015",
    "funding_timestamp": 1762863600000,
    "daily_base_token_volume": 125.5,
    "daily_quote_token_volume": 13177500.0,
    "daily_price_low": 104500.0,
    "daily_price_high": 106200.0,
    "daily_price_change": 1.5
  }
}
```

**Use Cases:**
- Risk management
- Position monitoring
- Funding rate strategies
- Market overview

**Update Frequency:** ~1-5 seconds

---

### Transaction (REMOVED)

**Channel:** `transaction`

**Status:** ❌ **REMOVED - Retired on November 11, 2025 @ 3 PM UTC**

**Replacement:** Use `account_all_orders` or `account_tx` channels

**Note:** This channel is no longer available and has been removed from the SDK.

---

### Executed Transaction (REMOVED)

**Channel:** `executed_transaction`

**Status:** ❌ **REMOVED - Retired on November 11, 2025 @ 3 PM UTC**

**Replacement:** Use `account_market_trades` for fills with client_order_id matching

**Note:** This channel is no longer available and has been removed from the SDK.

---

### Height

**Channel:** `height`

**Subscribe Format:**
```json
{"type": "subscribe", "channel": "height"}
```

**Data Provided:**
- Blockchain height updates
- Useful for tracking block confirmations

**Response:**
```json
{
  "channel": "height",
  "height": 12345678,
  "type": "height"
}
```

**Use Cases:**
- Block confirmation tracking
- Transaction finality monitoring

---

## Private Channels

Private channels require authentication via auth token.

### Account All

**Channel:** `account_all/{ACCOUNT_ID}`

**Data Provided:**
- Account data across ALL markets
- Positions (all markets)
- Trades (all markets)
- Pool shares
- Funding histories
- Trade volume statistics

**Response Structure:**
```json
{
  "account": 281474976665656,
  "channel": "account_all/281474976665656",
  "daily_trades_count": 42,
  "weekly_trades_count": 215,
  "monthly_trades_count": 850,
  "total_trades_count": 5432,
  "daily_volume": 50000.0,
  "weekly_volume": 250000.0,
  "monthly_volume": 1000000.0,
  "total_volume": 5000000.0,
  "funding_histories": {
    "0": [/* funding events for market 0 */],
    "1": [/* funding events for market 1 */]
  },
  "positions": {
    "0": {/* position object */},
    "1": {/* position object */}
  },
  "shares": [/* pool shares array */],
  "trades": {
    "0": [/* trades for market 0 */],
    "1": [/* trades for market 1 */]
  }
}
```

**Use Cases:**
- Portfolio-wide monitoring
- Multi-market trading
- Account statistics tracking

---

### Account Market

**Channel:** `account_market/{MARKET_ID}/{ACCOUNT_ID}`

**Requires:** Authentication

**Data Provided:**
- Account data for ONE specific market
- Orders (active on this market)
- Position (for this market)
- Trades (on this market)
- Funding history (for this market)

**Response Structure:**
```json
{
  "account": 281474976665656,
  "channel": "account_market/1/281474976665656",
  "funding_history": {
    "timestamp": 1762860000000,
    "market_id": 1,
    "funding_id": 12345,
    "change": "-0.5",
    "rate": "0.0001",
    "position_size": "1.5",
    "position_side": "long"
  },
  "orders": [/* order objects */],
  "position": {/* position object */},
  "trades": [/* trade objects */]
}
```

**Use Cases:**
- Single-market focused bots
- Detailed market-specific monitoring

---

### User Stats (Account Stats)

**Channel:** `user_stats/{ACCOUNT_ID}`

**Data Provided:**
- Account collateral
- Portfolio value
- Leverage (current and cross)
- Available balance
- Margin usage
- Buying power

**Response Structure:**
```json
{
  "channel": "user_stats/281474976665656",
  "stats": {
    "collateral": "10000.0",
    "portfolio_value": "11250.5",
    "leverage": "2.5",
    "available_balance": "5000.0",
    "margin_usage": "50.0",
    "buying_power": "25000.0",
    "cross_stats": {
      "collateral": "10000.0",
      "portfolio_value": "11250.5",
      "leverage": "2.5",
      "available_balance": "5000.0",
      "margin_usage": "50.0",
      "buying_power": "25000.0"
    },
    "total_stats": {/* same structure */}
  }
}
```

**Use Cases:**
- Risk management
- Leverage monitoring
- Available capital tracking
- Margin call prevention

---

### Account Tx

**Channel:** `account_tx/{ACCOUNT_ID}`

**Requires:** Authentication

**Data Provided:**
- All transactions related to your account
- Filters transaction channel to only your transactions

**Use Cases:**
- Transaction history
- Order placement confirmations
- Deposit/withdrawal tracking

---

### Account All Orders

**Channel:** `account_all_orders/{ACCOUNT_ID}`

**Requires:** Authentication

**Subscribe (Rust SDK):**
```rust
.subscribe_account_all_orders(account_id)
```

**Event Type:** `WsEvent::Account(AccountEventEnvelope)`

**Data Provided:**
- Order creation confirmations
- Order cancellation confirmations
- Order status updates
- Fill notifications
- Orders across ALL markets

**Response Structure:**
```json
{
  "channel": "account_all_orders/281474976665656",
  "orders": {
    "0": [/* orders for market 0 */],
    "1": [
      {
        "order_index": 562949953421312,
        "account_id": 281474976665656,
        "market_id": 1,
        "price": "105000.0",
        "size": "0.001",
        "is_buy": true,
        "order_type": "limit",
        "timestamp": 1762860000000,
        "client_order_id": 12345
      }
    ]
  }
}
```

**Use Cases:**
- Multi-market order tracking
- Portfolio-wide order management
- Order lifecycle monitoring

---

### Account Orders

**Channel:** `account_orders/{MARKET_INDEX}/{ACCOUNT_ID}`

**Requires:** Authentication

**Data Provided:**
- Orders for ONE specific market only
- Same order structure as account_all_orders
- Includes nonce field for sequencing

**Response Structure:**
```json
{
  "account": 281474976665656,
  "channel": "account_orders/1/281474976665656",
  "nonce": 12345,
  "orders": {
    "1": [/* order objects for market 1 */]
  }
}
```

**Use Cases:**
- Single-market order tracking
- Market-specific bots

---

### Account All Trades

**Channel:** `account_all_trades/{ACCOUNT_ID}`

**Requires:** Authentication

**Subscribe (Rust SDK):**
```rust
.subscribe_account_all_trades(account_id)
```

**Event Type:** `WsEvent::Account(AccountEventEnvelope)`

**Data Provided:**
- YOUR fills across ALL markets
- Volume statistics (daily, weekly, monthly, total)

**Response Structure:**
```json
{
  "channel": "account_all_trades/281474976665656",
  "trades": {
    "0": [/* ETH trades */],
    "1": [/* BTC trades */],
    "2": [/* SOL trades */]
  },
  "daily_volume": 211.528551,
  "weekly_volume": 585731.597846,
  "monthly_volume": 1115281.414524,
  "total_volume": 1905277.336754
}
```

**Use Cases:**
- Multi-market trading
- Portfolio tracking
- Volume monitoring
- Account-wide PnL

---

### Account Market Trades

**Channel:** `account_market_trades/{MARKET_INDEX}/{ACCOUNT_ID}`

**Requires:** Authentication

**Subscribe (Rust SDK):**
```rust
.subscribe_account_market_trades(market_id, account_id)
```

**Event Type:** `WsEvent::Account(AccountEventEnvelope)`

**Data Provided:**
- YOUR fills on ONE specific market
- Trade details with YOUR side
- **NEW:** ask_client_id and bid_client_id fields
- Position updates

**Response Structure:**
```json
{
  "channel": "account_trades/1/281474976665656",
  "trades": {
    "1": [
      {
        "trade_id": 449491731,
        "market_id": 1,
        "price": "105266.1",
        "size": "0.00001",
        "timestamp": 1762860622253,

        "ask_account_id": 257024,
        "ask_client_id": 264308218600602,  // Seller's client_order_id
        "ask_id": 562950522978024,         // Seller's order_index

        "bid_account_id": 281474976665656,
        "bid_client_id": 0,                // Buyer's client_order_id (0 if not set)
        "bid_id": 844424346732582,         // Buyer's order_index

        "is_maker_ask": true,
        "maker_fee": 20,
        "taker_fee": 200,
        "usd_amount": "1.052661",

        "taker_position_size_before": "0.00001",
        "maker_position_size_before": "-0.07079"
      }
    ]
  }
}
```

**Key Fields:**

| Field | Description | Example |
|-------|-------------|---------|
| `ask_client_id` | Seller's custom order ID | `264308218600602` or `0` |
| `bid_client_id` | Buyer's custom order ID | `12345` or `0` |
| `ask_id` | Seller's order_index (always exists) | `562950522978024` |
| `bid_id` | Buyer's order_index (always exists) | `844424346732582` |
| `is_maker_ask` | True if seller was passive (maker) | `true` or `false` |

**How to Match Your Orders:**

**Option 1: Use client_order_id** (if you set it)
```rust
let my_client_id = 12345;

if my_account_is_buyer {
    if trade.bid_client_id == my_client_id {
        println!("My order {} filled!", my_client_id);
    }
} else {
    if trade.ask_client_id == my_client_id {
        println!("My order {} filled!", my_client_id);
    }
}
```

**Option 2: Use order_index** (always works)
```rust
let my_order_index = 562950522978024;

if my_account_is_buyer {
    if trade.bid_id == my_order_index {
        println!("My order filled!");
    }
} else {
    if trade.ask_id == my_order_index {
        println!("My order filled!");
    }
}
```

**Use Cases:**
- Detecting fills on specific market
- Order matching (which order filled)
- Position tracking
- PnL calculation
- Multiple bot coordination (using client_order_id)

---

### Account All Positions

**Channel:** `account_all_positions/{ACCOUNT_ID}`

**Requires:** Authentication

**Data Provided:**
- All open positions across all markets
- Pool shares

**Response Structure:**
```json
{
  "channel": "account_all_positions/281474976665656",
  "positions": {
    "0": {
      "market_id": 0,
      "size": "1.5",
      "side": "long",
      "entry_price": "3200.0",
      "mark_price": "3250.5",
      "liquidation_price": "2800.0",
      "unrealized_pnl": "75.75"
    },
    "1": {/* BTC position */}
  },
  "shares": [
    {
      "pool_id": 123,
      "shares": 1000,
      "value": "5000.0"
    }
  ]
}
```

**Use Cases:**
- Portfolio monitoring
- Risk management
- Position tracking across markets

---

### Pool Data

**Channel:** `pool_data/{ACCOUNT_ID}`

**Requires:** Authentication

**Data Provided:**
- Complete pool trading data
- Trades (per market)
- Orders (per market)
- Positions (per market)
- Pool shares
- Funding histories (per market)

**Response Structure:**
```json
{
  "channel": "pool_data/281474976665656",
  "account": 281474976665656,
  "trades": {
    "0": [/* market 0 trades */],
    "1": [/* market 1 trades */]
  },
  "orders": {
    "0": [/* market 0 orders */],
    "1": [/* market 1 orders */]
  },
  "positions": {
    "0": {/* market 0 position */},
    "1": {/* market 1 position */}
  },
  "shares": [/* pool shares array */],
  "funding_histories": {
    "0": [/* market 0 funding */],
    "1": [/* market 1 funding */]
  }
}
```

**Use Cases:**
- Pool management
- LP tracking
- Complete pool state monitoring

---

### Pool Info

**Channel:** `pool_info/{ACCOUNT_ID}`

**Requires:** Authentication

**Data Provided:**
- Pool status and configuration
- Operator fee settings
- Total shares and operator shares
- APY (Annual Percentage Yield)
- Daily returns history
- Share price history

**Response Structure:**
```json
{
  "channel": "pool_info/281474976665656",
  "pool_info": {
    "status": 1,
    "operator_fee": "0.1",
    "min_operator_share_rate": "0.05",
    "total_shares": 10000,
    "operator_shares": 1000,
    "annual_percentage_yield": 15.5,
    "daily_returns": [
      {"timestamp": 1762860000000, "daily_return": 0.042},
      {"timestamp": 1762773600000, "daily_return": 0.038}
    ],
    "share_prices": [
      {"timestamp": 1762860000000, "share_price": 1.05},
      {"timestamp": 1762773600000, "share_price": 1.048}
    ]
  }
}
```

**Use Cases:**
- Pool performance tracking
- APY monitoring
- Share price tracking
- LP analytics

---

### Notification

**Channel:** `notification/{ACCOUNT_ID}`

**Requires:** Authentication

**Data Provided:**
- Liquidation notifications
- Deleverage notifications
- System announcements

**Response Structure:**
```json
{
  "channel": "notification/281474976665656",
  "notifs": [
    {
      "id": 12345,
      "created_at": 1762860000000,
      "updated_at": 1762860000000,
      "kind": "liquidation",
      "account_index": 281474976665656,
      "content": {
        "id": 789,
        "is_ask": true,
        "usdc_amount": "5000.0",
        "size": "0.05",
        "market_index": 1,
        "price": "100000.0",
        "timestamp": 1762860000000,
        "avg_price": "100000.0"
      },
      "ack": false,
      "acked_at": null
    },
    {
      "id": 12346,
      "kind": "deleverage",
      "content": {
        "id": 790,
        "usdc_amount": "3000.0",
        "size": "0.03",
        "market_index": 0,
        "settlement_price": "100000.0",
        "timestamp": 1762861000000
      }
    },
    {
      "id": 12347,
      "kind": "announcement",
      "content": {
        "title": "System Upgrade",
        "content": "Scheduled maintenance on Nov 12",
        "created_at": 1762862000000
      }
    }
  ]
}
```

**Notification Types:**

1. **Liquidation:**
   - Your position was liquidated
   - Shows price, size, market

2. **Deleverage:**
   - Your position was deleveraged
   - Shows settlement price, size

3. **Announcement:**
   - System announcements
   - Important updates

**Use Cases:**
- Risk alerts
- Position monitoring
- System status tracking

---

## Connection Management

### Basic Setup (Rust SDK)

```rust
let mut stream = client
    .ws()
    .subscribe_order_book(market_id)
    .subscribe_bbo(market_id)
    .subscribe_account_market_trades(market_id, account_id)
    .connect()
    .await?;

// Set auth token for private channels
let auth_token = client.create_auth_token(None)?;
stream.connection_mut().set_auth_token(auth_token);
```

### Event Loop

```rust
while let Some(event) = stream.next().await {
    match event? {
        WsEvent::Connected => {
            println!("✅ Connected");
        }
        WsEvent::OrderBook(update) => {
            // Handle order book
        }
        WsEvent::Account(envelope) => {
            // Handle account events
        }
        WsEvent::Pong => {
            // Keep-alive response
        }
        WsEvent::Closed(frame) => {
            println!("⚠️ Connection closed: {:?}", frame);
            break;
        }
        _ => {}
    }
}
```

### Reconnection Logic

```rust
loop {
    let result = run_trading_loop(&client, market_id, account_id).await;

    match result {
        Ok(_) => break,
        Err(e) => {
            println!("⚠️ Connection error: {}. Reconnecting...", e);
            tokio::time::sleep(Duration::from_secs(5)).await;
            // Loop will reconnect
        }
    }
}
```

### Handling Gaps (Premium)

```rust
let mut last_nonce: Option<u64> = None;

WsEvent::OrderBook(update) => {
    if let Some(nonce) = update.nonce {
        if let Some(prev_nonce) = last_nonce {
            if nonce != prev_nonce + 1 {
                println!("⚠️ Gap detected! Missed {} updates",
                         nonce - prev_nonce - 1);

                // Request fresh snapshot via REST
                let snapshot = client.orders().book(market_id, 50).await?;
                reset_order_book(snapshot);
            }
        }
        last_nonce = Some(nonce);
    }
}
```

---

## Performance Tips

### Bandwidth Optimization

**High Bandwidth (Full Data):**
```rust
.subscribe_order_book(market_id)     // Full book, many updates
.subscribe_trade(market_id)          // All trades
```

**Low Bandwidth (Essential Only):**
```rust
.subscribe_bbo(market_id)            // Only TOB changes (~80% reduction)
.subscribe_account_market_trades(market_id, account) // Only your fills
```

### Latency Considerations

| Latency | Expected Reliability | Recommendation |
|---------|---------------------|----------------|
| 1-5ms (colocation) | 95-99% | Use full order book |
| 10-50ms (same region) | 85-95% | Use order book + nonce checking |
| 100-200ms (cross-region) | 30-70% | Use BBO + frequent REST snapshots |
| >200ms | <30% | Consider REST polling instead |

### Memory Management

- Order book state: ~5-10 MB per market
- BBO only: ~1 KB
- Trade history: Grows over time, implement rotation

---

## Common Patterns

### Market Making Bot

```rust
.subscribe_order_book(market_id)              // For price levels
.subscribe_account_all_orders(account_id)      // For order status
.subscribe_account_market_trades(market_id, account) // For fills
```

### Simple Trading Bot

```rust
.subscribe_bbo(market_id)                     // For price
.subscribe_account_market_trades(market_id, account) // For fills
```

### Multi-Market Portfolio Tracker

```rust
.subscribe_bbo(btc_market)
.subscribe_bbo(eth_market)
.subscribe_bbo(sol_market)
.subscribe_account_all_trades(account_id)     // All fills + volume stats
```

### Risk Monitor

```rust
.subscribe_market_stats(market_id)            // For mark price, OI
.subscribe_user_stats(account_id)             // For margin, leverage
.subscribe_notification(account_id)           // For liquidation alerts
```

### Pool Management

```rust
.subscribe_pool_data(account_id)              // Complete pool data
.subscribe_pool_info(account_id)              // APY and performance
```

---

## Troubleshooting

### No Events Received

**Check:**
1. Auth token set for private channels
2. Account ID correct
3. Market ID valid
4. Connection not closed

### Missing Trades

**Possible Causes:**
- Using `subscribe_account_market_trades()` but no orders filled
- Wrong account_id or market_id
- Orders placed without fills

**Solution:**
- Check order book to see if your orders are active
- Use `subscribe_account_all_orders()` to confirm order placement

### High Packet Loss

**Symptoms:**
- Large nonce gaps
- Reliability < 90%

**Solutions:**
1. Move to colocation (1-5ms latency)
2. Switch to BBO channel (less data)
3. Implement REST snapshot refresh on gaps
4. Use account_market_trades only (no public channels)

### client_order_id is Always 0

**Reason:** You're not setting it when creating orders

**Solution:**
```rust
.create_limit_order(market, size, price, is_buy)
.with_client_order_id(12345)  // Set custom ID
.execute()
.await?;
```

Or use auto-generation:
```rust
.auto_client_id()  // SDK generates unique ID
```

---

## Best Practices

1. **Always check account_id** when processing Account events
2. **Use nonce checking** (Premium) for reliable order books
3. **Set client_order_id** for multi-bot setups
4. **Implement reconnection logic** with exponential backoff
5. **Monitor connection health** (track last message time)
6. **Use BBO for high-latency** connections (>50ms)
7. **Request REST snapshots** when gaps detected
8. **Avoid subscribing** to channels you don't need
9. **Subscribe to notification channel** for liquidation alerts
10. **Use account_all_* channels** for portfolio-wide monitoring

---

## Quick Reference

| Need | Subscribe To |
|------|-------------|
| Current price | `order_book/{market}` or `bbo/{market}` |
| Full order book | `order_book/{market}` |
| Public trades | `trade/{market}` |
| My fills (one market) | `account_market_trades/{market}/{account}` |
| My fills (all markets) | `account_all_trades/{account}` |
| My orders (one market) | `account_orders/{market}/{account}` |
| My orders (all markets) | `account_all_orders/{account}` |
| Account stats | `user_stats/{account}` |
| Funding rate, OI | `market_stats/{market}` |
| All positions | `account_all_positions/{account}` |
| Liquidation alerts | `notification/{account}` |
| Portfolio overview | `account_all/{account}` |
| Pool performance | `pool_info/{account}` |
| Block height | `height` |

---

## Channel Summary Table

| Channel | Auth Required | Data Scope | Use Case |
|---------|--------------|------------|----------|
| order_book | No | Market | Full order book depth |
| bbo | No | Market | Best bid/ask only |
| trade | No | Market | Public trades |
| market_stats | No | Market | Price, funding, OI |
| height | No | Global | Block height |
| account_all | No | Account (all markets) | Portfolio overview |
| account_market | Yes | Account (one market) | Market-specific data |
| user_stats | No | Account | Collateral, leverage |
| account_tx | Yes | Account | Transactions |
| account_all_orders | Yes | Account (all markets) | All orders |
| account_orders | Yes | Account (one market) | Market orders |
| account_all_trades | Yes | Account (all markets) | All fills + volume |
| account_market_trades | Yes | Account (one market) | Market fills |
| account_all_positions | Yes | Account (all markets) | All positions |
| pool_data | Yes | Pool | Complete pool data |
| pool_info | Yes | Pool | APY, performance |
| notification | Yes | Account | Alerts, announcements |

---

## Version History

- **v0.3.0** (Nov 11, 2025)
  - **REMOVED** `subscribe_transactions()` and `subscribe_executed_transactions()` - channels retired by Lighter
  - Use `account_all_orders` and `account_market_trades` with client_order_id matching instead

- **v0.2.0** (Nov 11, 2025)
  - Added `ask_client_id` / `bid_client_id` fields on trades
  - Added `nonce` field on order book (Premium)
  - Added `volume_quota_remaining` on transactions
  - Documented all 17 official WebSocket channels

---

**Last Updated:** November 11, 2025
**Source:** https://apidocs.lighter.xyz/docs/websocket-reference
