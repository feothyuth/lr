# Error Handling Guide

This guide explains how to properly handle errors when using the Lighter Client Rust SDK, with special focus on order submission and rejection scenarios.

## Table of Contents

- [Overview](#overview)
- [Error Types](#error-types)
- [Order Rejection Errors](#order-rejection-errors)
- [Common Rejection Reasons](#common-rejection-reasons)
- [Best Practices](#best-practices)
- [Examples](#examples)

## Overview

The Lighter Client SDK provides comprehensive error handling that distinguishes between:
1. **Client-side validation errors** - Caught before submission
2. **Network/transport errors** - Connection issues
3. **Exchange rejection errors** - Orders rejected by the exchange

Starting from version 0.x.x, the SDK validates exchange responses and throws detailed errors when orders are rejected, preventing silent failures.

## Error Types

### Main Error Types

The SDK uses two primary error enums:

#### `lighter_client::errors::Error`

Used by the high-level `LighterClient` API:

```rust
pub enum Error {
    /// Errors from the signing client
    Signer(SignerClientError),

    /// WebSocket client errors
    Ws(WsClientError),

    /// Configuration validation failures
    InvalidConfig { field: &'static str, why: &'static str },

    /// Not authenticated for this operation
    NotAuthenticated,

    /// Rate limited by server
    RateLimited { retry_after: Option<u64> },

    /// Structured server error
    Server { status: u16, message: String, code: Option<String> },

    /// Raw HTTP error
    Http { status: u16, body: String },
}
```

#### `errors::SignerClientError`

Used by the low-level signing and transaction submission:

```rust
pub enum SignerClientError {
    /// Signer library error (Go FFI)
    Signer(String),

    /// Nonce sequencing error
    Nonce(String),

    /// HTTP error
    Http(String),

    /// Cryptographic signing error
    Signing(String),

    /// Invalid input validation
    InvalidInput(String),

    /// **NEW**: Order rejected by exchange
    OrderRejected {
        code: i32,
        message: String,
        tx_hash: String,
    },

    // ... other variants
}
```

## Order Rejection Errors

### What is an OrderRejected Error?

When you submit an order, the exchange performs validation checks. If the order fails these checks, it returns an error response with:
- `code`: HTTP-style error code (e.g., 400 for validation errors)
- `message`: Human-readable explanation of why the order was rejected
- `tx_hash`: Transaction hash (generated even for rejected orders, for debugging)

**Important**: Prior to this update, rejected orders would appear successful because the SDK only checked for network errors, not business logic failures.

### How to Handle OrderRejected Errors

```rust
use lighter_client::{
    lighter_client::LighterClient,
    types::{MarketId, BaseQty, Price, Expiry},
};
use time::Duration;

match client
    .order(MarketId::new(0))
    .buy()
    .qty(BaseQty::try_from(100)?)
    .limit(Price::ticks(115_000))
    .expires_at(Expiry::from_now(Duration::minutes(10)))
    .post_only()
    .submit()
    .await
{
    Ok(submission) => {
        println!("Order submitted successfully!");
        println!("TX Hash: {}", submission.response().tx_hash);
    }
    Err(e) => {
        // Pattern match on the error type
        match e {
            lighter_client::errors::Error::Signer(signer_err) => {
                match signer_err {
                    lighter_client::errors::SignerClientError::OrderRejected {
                        code,
                        message,
                        tx_hash
                    } => {
                        eprintln!("Order rejected by exchange!");
                        eprintln!("  Code: {}", code);
                        eprintln!("  Reason: {}", message);
                        eprintln!("  TX Hash (for debugging): {}", tx_hash);
                    }
                    other => {
                        eprintln!("Signer error: {}", other);
                    }
                }
            }
            other => {
                eprintln!("Submission failed: {}", other);
            }
        }
    }
}
```

## Common Rejection Reasons

### 1. Expiry Too Short (< 10 Minutes)

**Error**: `"expiry must be at least 10 minutes"`

**Cause**: Good-Till-Time (GTT) and Post-Only orders require a minimum 10-minute expiry.

**Solution**:
```rust
// ❌ WRONG - Will be rejected
.expires_at(Expiry::from_now(Duration::minutes(5)))

// ✅ CORRECT - Use minimum 10 minutes
.expires_at(Expiry::from_now(Duration::minutes(10)))
```

**Client-Side Validation**: The SDK now catches this error BEFORE submission:
```rust
Err(Error::InvalidConfig {
    field: "expiry",
    why: "must be at least 10 minutes in the future for GTT/PostOnly orders",
})
```

### 2. Price Outside Fat-Finger Bounds

**Error**: `"order price flagged as an accidental price"` or `"limit order price is too far from the mark price"`

**Cause**: Lighter enforces price bounds to prevent fat-finger errors:
- **Buy orders**: Price must be ≤ 105% of min(MarkPrice, BestAsk)
- **Sell orders**: Price must be ≥ 95% of max(MarkPrice, BestBid)

**Solution**:
```rust
// Query current market prices first
let order_book = client.orders().book(market_id, 10).await?;
let mark_price = 115_000; // Get from market data

let best_ask = order_book.asks.first().unwrap().price;
let max_buy_price = (best_ask * 1.05) as i64;

// Ensure your limit price is within bounds
let safe_buy_price = std::cmp::min(your_price, max_buy_price);

client.order(market_id)
    .buy()
    .qty(qty)
    .limit(Price::ticks(safe_buy_price))
    .submit()
    .await?;
```

### 3. Post-Only Order Would Cross Spread

**Error**: Order silently cancelled (no explicit error message)

**Cause**: Post-only orders are designed to ONLY add liquidity (be maker orders). If your price would immediately match an existing order (take liquidity), the order is cancelled.

**Example**:
- Best Ask: 115,000
- Your post-only buy at 115,000 or higher → ❌ Rejected (would match immediately)
- Your post-only buy at 114,999 or lower → ✅ Accepted (adds to bid side)

**Solution**:
```rust
// Query orderbook first
let book = client.orders().book(market, 1).await?;
let best_ask = book.asks.first().unwrap().price;

// For post-only buy, ensure price is BELOW best ask
let safe_price = best_ask - 1; // At least 1 tick below

client.order(market)
    .buy()
    .qty(qty)
    .limit(Price::ticks(safe_price))
    .post_only()
    .submit()
    .await?;
```

**Alternative**: Remove `.post_only()` if you're willing to pay taker fees:
```rust
client.order(market)
    .buy()
    .qty(qty)
    .limit(Price::ticks(best_ask)) // Can now match immediately
    .submit() // No post_only()
    .await?;
```

### 4. Insufficient Margin

**Error**: `"insufficient margin"` or `"order would cause account to become unhealthy"`

**Cause**: Your account doesn't have enough collateral to support the order.

**Solution**:
```rust
// Check account margin before submitting
let account_info = client.account().info().await?;

// Calculate required margin for your order
let order_size = 100;
let order_price = 115_000;
let required_margin = calculate_required_margin(order_size, order_price);

if account_info.available_margin < required_margin {
    eprintln!("Insufficient margin. Need {}, have {}",
              required_margin, account_info.available_margin);
    return;
}

// Proceed with order submission
```

### 5. Order Size Below Minimum

**Error**: `"order size below minimum"`

**Cause**: Each market has a minimum order size requirement.

**Solution**:
```rust
// Query market constraints
let markets = client.orders().order_books(Some(market_id)).await?;
let market_info = &markets.data[0];

let min_size = market_info.min_base_amount.parse::<i64>()?;

if your_size < min_size {
    eprintln!("Order size too small. Minimum: {}, yours: {}", min_size, your_size);
    return Err("Size too small".into());
}
```

### 6. Invalid Tick Size

**Error**: `"price must be multiple of tick size"`

**Cause**: Order prices must align with the market's tick size.

**Solution**:
```rust
let markets = client.orders().order_books(Some(market_id)).await?;
let market_info = &markets.data[0];

let tick_size = market_info.price_tick_size;

// Round price to nearest valid tick
let rounded_price = (your_price / tick_size) * tick_size;

client.order(market_id)
    .buy()
    .qty(qty)
    .limit(Price::ticks(rounded_price))
    .submit()
    .await?;
```

## Best Practices

### 1. Always Handle Errors Explicitly

**❌ DON'T** use `?` operator without handling:
```rust
let submission = client.order(market).buy().qty(qty).submit().await?;
// Silent failure if order is rejected!
```

**✅ DO** use pattern matching:
```rust
match client.order(market).buy().qty(qty).submit().await {
    Ok(submission) => { /* handle success */ }
    Err(e) => { /* handle error explicitly */ }
}
```

### 2. Pre-Validate When Possible

The SDK now performs client-side validation for some common issues:

```rust
// This will fail BEFORE submission (fast, no network call)
client.order(market)
    .buy()
    .qty(qty)
    .expires_at(Expiry::from_now(Duration::minutes(5))) // ❌ Too short
    .submit()
    .await // Returns InvalidConfig error immediately
```

### 3. Query Market Constraints First

```rust
async fn submit_safe_order(
    client: &LighterClient,
    market_id: MarketId,
    size: i64,
    price: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Get market info
    let markets = client.orders().order_books(Some(market_id.as_i32())).await?;
    let market = &markets.data[0];

    // 2. Validate size
    let min_size = market.min_base_amount.parse::<i64>()?;
    if size < min_size {
        return Err(format!("Size {} below minimum {}", size, min_size).into());
    }

    // 3. Validate price ticks
    let price = (price / market.price_tick_size) * market.price_tick_size;

    // 4. Check price bounds
    let book = client.orders().book(market_id.as_i32(), 1).await?;
    let mark_price = 115_000; // Get from market stats
    let best_ask = book.asks.first().map(|o| o.price).unwrap_or(mark_price);
    let max_buy_price = (std::cmp::min(mark_price, best_ask as i64) as f64 * 1.05) as i64;

    if price > max_buy_price {
        return Err(format!("Price {} exceeds maximum {}", price, max_buy_price).into());
    }

    // 5. Submit order
    let submission = client
        .order(market_id)
        .buy()
        .qty(BaseQty::try_from(size)?)
        .limit(Price::ticks(price))
        .expires_at(Expiry::from_now(Duration::minutes(10)))
        .post_only()
        .submit()
        .await?;

    println!("Order submitted: {}", submission.response().tx_hash);
    Ok(())
}
```

### 4. Implement Retry Logic for Transient Errors

```rust
use tokio::time::{sleep, Duration};

async fn submit_with_retry(
    client: &LighterClient,
    market_id: MarketId,
    qty: BaseQty,
    price: Price,
    max_retries: u32,
) -> Result<Submission<CreateOrder>, Box<dyn std::error::Error>> {
    let mut attempts = 0;

    loop {
        match client
            .order(market_id)
            .buy()
            .qty(qty)
            .limit(price)
            .expires_at(Expiry::from_now(Duration::minutes(10)))
            .submit()
            .await
        {
            Ok(submission) => return Ok(submission),
            Err(e) => {
                attempts += 1;

                // Check if error is retryable
                let is_retryable = matches!(
                    e,
                    Error::RateLimited { .. } | Error::Http { status: 500..=599, .. }
                );

                if !is_retryable || attempts >= max_retries {
                    return Err(e.into());
                }

                eprintln!("Attempt {} failed: {}. Retrying...", attempts, e);
                sleep(Duration::from_secs(2_u64.pow(attempts))).await; // Exponential backoff
            }
        }
    }
}
```

### 5. Log TX Hashes Even for Failures

Even rejected orders return a `tx_hash` for debugging:

```rust
match submission_result {
    Ok(submission) => {
        info!("Order succeeded: {}", submission.response().tx_hash);
    }
    Err(Error::Signer(SignerClientError::OrderRejected { tx_hash, message, .. })) => {
        error!("Order rejected: {}", message);
        error!("TX Hash for debugging: {}", tx_hash);
        // You can query this TX hash via API to investigate
    }
    Err(e) => {
        error!("Submission failed: {}", e);
    }
}
```

## Examples

### Example 1: Basic Error Handling

```rust
use lighter_client::{
    lighter_client::LighterClient,
    types::{MarketId, BaseQty, Price, Expiry},
};
use time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = build_client().await?;

    let result = client
        .order(MarketId::new(0))
        .buy()
        .qty(BaseQty::try_from(100)?)
        .limit(Price::ticks(115_000))
        .expires_at(Expiry::from_now(Duration::minutes(10)))
        .post_only()
        .submit()
        .await;

    match result {
        Ok(submission) => {
            println!("✅ Order submitted!");
            println!("TX Hash: {}", submission.response().tx_hash);
        }
        Err(e) => {
            eprintln!("❌ Order failed: {}", e);
            // Provide helpful guidance based on error type
            if format!("{:?}", e).contains("OrderRejected") {
                eprintln!("\nCommon solutions:");
                eprintln!("  - Ensure expiry is ≥10 minutes");
                eprintln!("  - Check price is within ±5% of market");
                eprintln!("  - Verify post-only order won't cross spread");
            }
        }
    }

    Ok(())
}
```

### Example 2: Comprehensive Production Handler

See `examples/submit_test_order.rs` for a complete example with:
- Detailed error matching
- Helpful error messages
- Common rejection reason explanations
- TX hash logging for debugging

## Migration Guide

### For Existing Code

**Before** (versions < 0.x.x):
```rust
// Silent failures - orders could be rejected but appear successful
let submission = client.order(market).buy().qty(qty).submit().await?;
println!("Success! TX: {}", submission.response().tx_hash);
// But order might not actually be on the book!
```

**After** (versions ≥ 0.x.x):
```rust
// Explicit error handling - rejections throw errors
match client.order(market).buy().qty(qty).submit().await {
    Ok(submission) => {
        println!("Confirmed success! TX: {}", submission.response().tx_hash);
        // Order is definitely on the book
    }
    Err(e) => {
        eprintln!("Order rejected: {}", e);
        // Handle rejection appropriately
    }
}
```

### Breaking Changes

1. **Orders are no longer silently rejected**
   - Previously: `Ok(Submission)` even if exchange rejected the order
   - Now: `Err(OrderRejected)` when exchange rejects the order

2. **Client-side validation added**
   - Expiry < 10 minutes now throws `InvalidConfig` before submission
   - Reduces unnecessary network calls for invalid orders

3. **Error types updated**
   - New variant: `SignerClientError::OrderRejected`
   - Wrapped in `Error::Signer` when returned from high-level API

## Related Documentation

- [Order Types](https://docs.lighter.xyz/perpetual-futures/order-types-and-matching)
- [API Reference](https://apidocs.lighter.xyz)
- [Examples](../examples/)

## Support

If you encounter issues not covered in this guide:
1. Check the [GitHub Issues](https://github.com/your-repo/issues)
2. Review the [API Documentation](https://apidocs.lighter.xyz)
3. Join the [Discord Community](https://discord.gg/lighter)
