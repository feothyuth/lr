# Inventory Skew Adjuster

An advanced market making example that demonstrates position-aware quoting for risk management on Lighter DEX.

## Overview

This example shows how to adjust market making quotes based on your current inventory position. The skew logic helps manage directional risk by:

- **LONG Position**: Widens bids and tightens asks to encourage selling
- **SHORT Position**: Tightens bids and widens asks to encourage buying
- **Skew Intensity**: Proportional to position size relative to maximum

## Features

1. **Position Monitoring**: Fetches current position via REST API
2. **Real-time BBO**: Subscribes to Best Bid/Offer via WebSocket
3. **Normal Quotes**: Calculates symmetric spread around mid-price
4. **Skewed Quotes**: Applies position-based adjustments
5. **Side-by-side Comparison**: Shows both normal and skewed quotes with percentages

## Configuration

### Environment Variables

```bash
LIGHTER_PRIVATE_KEY="your_private_key"      # Required: Your wallet private key
ACCOUNT_INDEX="0"                           # Required: Your account index
LIGHTER_API_KEY_INDEX="0"                   # Optional: API key index (default: 0)
LIGHTER_MARKET_ID="1"                       # Optional: Market ID (default: 1 = BTC-PERP)
```

### Skew Parameters (configurable in code)

```rust
const BASE_SPREAD: f64 = 0.002;           // 0.2% base spread
const MAX_POSITION_BTC: f64 = 0.01;       // Max position for normalization
const BID_SKEW_INTENSITY: f64 = -0.1;     // Bid adjustment intensity
const ASK_SKEW_INTENSITY: f64 = 0.05;     // Ask adjustment intensity
```

## Usage

```bash
# Run the example
LIGHTER_PRIVATE_KEY="..." ACCOUNT_INDEX="..." \
cargo run --example inventory_skew_adjuster
```

## Example Output

```
Position: +0.001 BTC (LONG) - 10.0% of max
BBO: $113,500.00 x $113,600.00
Mid: $113,550.00

Normal Quotes (0.2% spread):
  BID: $113,436.45 | ASK: $113,663.55

Skewed Quotes:
  BID: $113,423.08 (-0.010% skew) | ASK: $113,669.24 (+0.005% skew)

Skew Applied:
  LONG Position -> Bid widened (-0.010%), Ask tightened (+0.005%)
  Strategy: Encourage selling to reduce LONG exposure

Summary:
  Normal Spread: $227.10
  Skewed Spread: $246.16
  Bid Movement: $-13.37 (-0.01%)
  Ask Movement: $+5.69 (+0.01%)
```

## How It Works

### 1. Position Detection

```rust
let position_info = account.positions.iter()
    .find(|p| p.market_id == market_id);

let position_ratio = current_position / MAX_POSITION_BTC;
```

### 2. BBO Subscription

```rust
let mut stream = client.ws()
    .subscribe_bbo(market)
    .connect()
    .await?;
```

### 3. Normal Quote Calculation

```rust
let mid_price = (best_bid + best_ask) / 2.0;
let half_spread = BASE_SPREAD / 2.0;
let normal_bid = mid_price * (1.0 - half_spread);
let normal_ask = mid_price * (1.0 + half_spread);
```

### 4. Skew Application

For **LONG** positions (need to sell):
```rust
let bid_skew = BID_SKEW_INTENSITY * position_ratio;  // Negative (widen)
let ask_skew = ASK_SKEW_INTENSITY * position_ratio;  // Positive (tighten)
```

For **SHORT** positions (need to buy):
```rust
let bid_skew = -BID_SKEW_INTENSITY * position_ratio.abs();  // Positive (tighten)
let ask_skew = -ASK_SKEW_INTENSITY * position_ratio.abs();  // Negative (widen)
```

### 5. Apply Skew to Quotes

```rust
let skewed_bid = normal_bid * (1.0 + bid_skew);
let skewed_ask = normal_ask * (1.0 + ask_skew);
```

## Risk Management Strategy

### Why Skew Quotes?

Market makers face inventory risk when holding directional positions:

- **LONG inventory**: Risk of price dropping
  - **Solution**: Make it easier to sell (tighten ask) and harder to buy more (widen bid)
  
- **SHORT inventory**: Risk of price rising
  - **Solution**: Make it easier to buy (tighten bid) and harder to sell more (widen ask)

### Intensity Scaling

The skew intensity is proportional to position size:

- **Small position (10% of max)**: Small skew adjustment
- **Large position (100% of max)**: Maximum skew adjustment
- **No position**: No skew (symmetric quotes)

## Important Notes

1. **Calculation Only**: This example does NOT place actual orders
2. **Educational Purpose**: Demonstrates the math and logic of inventory skew
3. **Customizable**: Adjust skew parameters based on your risk tolerance
4. **Real-time Updates**: Shows continuous quote adjustments as BBO changes

## Integration with Market Making Bot

To integrate this logic into a real market making bot:

1. Use the skewed quotes as your order prices
2. Cancel and replace orders when position changes significantly
3. Adjust skew intensity based on market volatility
4. Consider maximum position limits for risk control
5. Monitor PnL impact of skewed vs. normal quotes

## Advanced Enhancements

Potential improvements for production use:

- **Dynamic spread**: Adjust base spread based on volatility
- **Multi-level quotes**: Apply skew to multiple price levels
- **Asymmetric size**: Vary order sizes based on position
- **Maximum skew limits**: Cap skew to prevent extreme quotes
- **Time-weighted position**: Consider how long you've held the position
- **Market impact**: Factor in expected fill probabilities

## References

- Lighter DEX SDK patterns: `examples/test_proper_close_position.rs`
- WebSocket BBO: `examples/06_websocket_all_channels.rs`
- Position parsing: `src/models/account_position.rs`

## Support

For questions or issues:
- SDK Documentation: Check `README.md`
- WebSocket Guide: See `WEBSOCKET_CHANNELS_GUIDE.md`
- Example: Run with `-h` for help
