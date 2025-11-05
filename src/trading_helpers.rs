//! Trading Helper Utilities for Lighter DEX
//!
//! This module provides convenient helper functions for common trading operations:
//! - Price calculations (mark-to-market, slippage, spread)
//! - Position size calculations
//! - Order book analysis
//! - Risk management utilities

use crate::errors::{Result, SignerClientError};

/// Calculate the mid-price from bid and ask prices
///
/// # Arguments
/// * `bid` - Best bid price
/// * `ask` - Best ask price
///
/// # Returns
/// Mid-price (bid + ask) / 2
pub fn calculate_mid_price(bid: f64, ask: f64) -> f64 {
    (bid + ask) / 2.0
}

/// Calculate the spread between bid and ask
///
/// # Arguments
/// * `bid` - Best bid price
/// * `ask` - Best ask price
///
/// # Returns
/// Absolute spread (ask - bid)
pub fn calculate_spread(bid: f64, ask: f64) -> f64 {
    ask - bid
}

/// Calculate the spread as a percentage of mid-price
///
/// # Arguments
/// * `bid` - Best bid price
/// * `ask` - Best ask price
///
/// # Returns
/// Spread percentage (spread / mid_price) * 100
pub fn calculate_spread_percentage(bid: f64, ask: f64) -> f64 {
    let mid = calculate_mid_price(bid, ask);
    let spread = calculate_spread(bid, ask);
    (spread / mid) * 100.0
}

/// Calculate price with slippage adjustment
///
/// # Arguments
/// * `base_price` - Base price before slippage
/// * `slippage_pct` - Slippage percentage (e.g., 0.015 for 1.5%)
/// * `is_buy` - True for buy orders (add slippage), false for sell orders (subtract slippage)
///
/// # Returns
/// Adjusted price with slippage
pub fn calculate_price_with_slippage(base_price: f64, slippage_pct: f64, is_buy: bool) -> f64 {
    if is_buy {
        base_price * (1.0 + slippage_pct)
    } else {
        base_price * (1.0 - slippage_pct)
    }
}

/// Calculate position value in USDC
///
/// # Arguments
/// * `position_size` - Position size in base currency
/// * `price` - Current price
///
/// # Returns
/// Position value (position_size * price)
pub fn calculate_position_value(position_size: f64, price: f64) -> f64 {
    position_size * price
}

/// Calculate unrealized PnL for a position
///
/// # Arguments
/// * `entry_price` - Entry price of the position
/// * `current_price` - Current mark price
/// * `position_size` - Position size (signed: positive for long, negative for short)
///
/// # Returns
/// Unrealized PnL in USDC
pub fn calculate_unrealized_pnl(entry_price: f64, current_price: f64, position_size: f64) -> f64 {
    (current_price - entry_price) * position_size
}

/// Calculate PnL percentage relative to entry value
///
/// # Arguments
/// * `entry_price` - Entry price of the position
/// * `current_price` - Current mark price
/// * `position_size` - Position size (absolute value)
///
/// # Returns
/// PnL percentage
pub fn calculate_pnl_percentage(entry_price: f64, current_price: f64, position_size: f64) -> f64 {
    let pnl = calculate_unrealized_pnl(entry_price, current_price, position_size);
    let entry_value = entry_price * position_size.abs();
    (pnl / entry_value) * 100.0
}

/// Calculate required margin for a position
///
/// # Arguments
/// * `position_value` - Position value in USDC
/// * `leverage` - Leverage multiplier (e.g., 10 for 10x)
///
/// # Returns
/// Required margin in USDC
pub fn calculate_required_margin(position_value: f64, leverage: f64) -> f64 {
    position_value / leverage
}

/// Calculate maximum position size given available margin
///
/// # Arguments
/// * `available_margin` - Available margin in USDC
/// * `price` - Current price
/// * `leverage` - Leverage multiplier
///
/// # Returns
/// Maximum position size in base currency
pub fn calculate_max_position_size(available_margin: f64, price: f64, leverage: f64) -> f64 {
    (available_margin * leverage) / price
}

/// Calculate liquidation price for a position
///
/// # Arguments
/// * `entry_price` - Entry price of the position
/// * `leverage` - Leverage multiplier
/// * `is_long` - True for long position, false for short position
/// * `maintenance_margin_pct` - Maintenance margin percentage (e.g., 0.05 for 5%)
///
/// # Returns
/// Approximate liquidation price
pub fn calculate_liquidation_price(
    entry_price: f64,
    leverage: f64,
    is_long: bool,
    maintenance_margin_pct: f64,
) -> f64 {
    let initial_margin_pct = 1.0 / leverage;
    let loss_tolerance_pct = initial_margin_pct - maintenance_margin_pct;

    if is_long {
        // Long: price can drop by loss_tolerance_pct
        entry_price * (1.0 - loss_tolerance_pct)
    } else {
        // Short: price can rise by loss_tolerance_pct
        entry_price * (1.0 + loss_tolerance_pct)
    }
}

/// Scale integer price to float with decimals
///
/// # Arguments
/// * `price_int` - Integer price from API
/// * `decimals` - Number of decimal places
///
/// # Returns
/// Float price
pub fn scale_price_from_int(price_int: i64, decimals: u8) -> f64 {
    price_int as f64 / 10_f64.powi(decimals as i32)
}

/// Scale float price to integer with decimals
///
/// # Arguments
/// * `price_float` - Float price
/// * `decimals` - Number of decimal places
///
/// # Returns
/// Integer price for API
pub fn scale_price_to_int(price_float: f64, decimals: u8) -> i64 {
    (price_float * 10_f64.powi(decimals as i32)).round() as i64
}

/// Scale integer size to float with decimals
///
/// # Arguments
/// * `size_int` - Integer size from API
/// * `decimals` - Number of decimal places
///
/// # Returns
/// Float size
pub fn scale_size_from_int(size_int: i64, decimals: u8) -> f64 {
    size_int as f64 / 10_f64.powi(decimals as i32)
}

/// Scale float size to integer with decimals
///
/// # Arguments
/// * `size_float` - Float size
/// * `decimals` - Number of decimal places
///
/// # Returns
/// Integer size for API
pub fn scale_size_to_int(size_float: f64, decimals: u8) -> i64 {
    (size_float * 10_f64.powi(decimals as i32)).round() as i64
}

/// Parse position string to float (handles API format)
///
/// # Arguments
/// * `position_str` - Position string from API (always positive)
///
/// # Returns
/// Result with float value or error
pub fn parse_position_size(position_str: &str) -> Result<f64> {
    position_str
        .parse::<f64>()
        .map_err(|e| SignerClientError::Signer(format!("Invalid position size: {}", e)))
}

/// Calculate grid levels for grid trading strategy
///
/// # Arguments
/// * `mid_price` - Mid price to center grid around
/// * `num_levels` - Number of levels on each side (buy and sell)
/// * `spacing_pct` - Spacing between levels as percentage (e.g., 0.005 for 0.5%)
///
/// # Returns
/// Tuple of (buy_prices, sell_prices)
pub fn calculate_grid_levels(
    mid_price: f64,
    num_levels: usize,
    spacing_pct: f64,
) -> (Vec<f64>, Vec<f64>) {
    let mut buy_prices = Vec::new();
    let mut sell_prices = Vec::new();

    for i in 1..=num_levels {
        let offset = spacing_pct * i as f64;
        buy_prices.push(mid_price * (1.0 - offset));
        sell_prices.push(mid_price * (1.0 + offset));
    }

    (buy_prices, sell_prices)
}

/// Calculate order size for grid trading with equal USDC value per level
///
/// # Arguments
/// * `total_capital` - Total capital to allocate
/// * `num_levels` - Number of levels
/// * `price` - Price for this level
///
/// # Returns
/// Order size in base currency for this level
pub fn calculate_grid_order_size(total_capital: f64, num_levels: usize, price: f64) -> f64 {
    let capital_per_level = total_capital / num_levels as f64;
    capital_per_level / price
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_mid_price() {
        assert_eq!(calculate_mid_price(100.0, 101.0), 100.5);
        assert_eq!(calculate_mid_price(3500.0, 3502.0), 3501.0);
    }

    #[test]
    fn test_calculate_spread() {
        assert_eq!(calculate_spread(100.0, 101.0), 1.0);
        assert_eq!(calculate_spread(3500.0, 3502.0), 2.0);
    }

    #[test]
    fn test_calculate_spread_percentage() {
        let spread_pct = calculate_spread_percentage(100.0, 101.0);
        assert!((spread_pct - 0.995).abs() < 0.001);
    }

    #[test]
    fn test_calculate_price_with_slippage() {
        // Buy with 1% slippage
        let buy_price = calculate_price_with_slippage(100.0, 0.01, true);
        assert_eq!(buy_price, 101.0);

        // Sell with 1% slippage
        let sell_price = calculate_price_with_slippage(100.0, 0.01, false);
        assert_eq!(sell_price, 99.0);
    }

    #[test]
    fn test_calculate_unrealized_pnl() {
        // Long position profit
        let pnl_long = calculate_unrealized_pnl(100.0, 110.0, 1.0);
        assert_eq!(pnl_long, 10.0);

        // Short position profit (negative position size)
        let pnl_short = calculate_unrealized_pnl(100.0, 90.0, -1.0);
        assert_eq!(pnl_short, 10.0);
    }

    #[test]
    fn test_calculate_required_margin() {
        let margin = calculate_required_margin(1000.0, 10.0);
        assert_eq!(margin, 100.0);
    }

    #[test]
    fn test_calculate_max_position_size() {
        let max_size = calculate_max_position_size(100.0, 50.0, 10.0);
        assert_eq!(max_size, 20.0);
    }

    #[test]
    fn test_calculate_liquidation_price() {
        // Long position with 10x leverage, 5% maintenance margin
        let liq_long = calculate_liquidation_price(100.0, 10.0, true, 0.05);
        assert!((liq_long - 95.0).abs() < 0.1);

        // Short position with 10x leverage, 5% maintenance margin
        let liq_short = calculate_liquidation_price(100.0, 10.0, false, 0.05);
        assert!((liq_short - 105.0).abs() < 0.1);
    }

    #[test]
    fn test_price_scaling() {
        // Scale from int to float
        let price_float = scale_price_from_int(350000, 2);
        assert_eq!(price_float, 3500.0);

        // Scale from float to int
        let price_int = scale_price_to_int(3500.0, 2);
        assert_eq!(price_int, 350000);
    }

    #[test]
    fn test_size_scaling() {
        // Scale from int to float
        let size_float = scale_size_from_int(10000, 4);
        assert_eq!(size_float, 1.0);

        // Scale from float to int
        let size_int = scale_size_to_int(1.0, 4);
        assert_eq!(size_int, 10000);
    }

    #[test]
    fn test_calculate_grid_levels() {
        let (buy_prices, sell_prices) = calculate_grid_levels(100.0, 3, 0.01);

        assert_eq!(buy_prices.len(), 3);
        assert_eq!(sell_prices.len(), 3);

        // Buy prices should be below mid
        assert_eq!(buy_prices[0], 99.0); // 1% below
        assert_eq!(buy_prices[1], 98.0); // 2% below
        assert_eq!(buy_prices[2], 97.0); // 3% below

        // Sell prices should be above mid
        assert_eq!(sell_prices[0], 101.0); // 1% above
        assert_eq!(sell_prices[1], 102.0); // 2% above
        assert_eq!(sell_prices[2], 103.0); // 3% above
    }

    #[test]
    fn test_calculate_grid_order_size() {
        let size = calculate_grid_order_size(1000.0, 10, 100.0);
        assert_eq!(size, 1.0); // $1000 / 10 levels / $100 = 1.0
    }
}
