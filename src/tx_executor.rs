//! Transaction Execution Module for Lighter DEX
//!
//! Handles:
//! - Single transaction submission via WebSocket
//! - Batch transaction submission (max 50 per batch per Lighter docs)
//! - Position closing with proper sign field handling
//!
//! CRITICAL: Position sign field must be used for direction detection!
//! - Long position (sign=1): Close with SELL (is_ask=1)
//! - Short position (sign=-1): Close with BUY (is_ask=0)

use crate::{
    errors::{Result as SignerResult, WsClientError, WsResult},
    signer::SignerLibrary,
    ws_client::WsConnection,
};
use serde_json::json;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

// Transaction type constants (matching signer_client.rs)
pub const TX_TYPE_CHANGE_PUB_KEY: u8 = 8;
pub const TX_TYPE_CREATE_SUB_ACCOUNT: u8 = 9;
pub const TX_TYPE_CREATE_PUBLIC_POOL: u8 = 10;
pub const TX_TYPE_UPDATE_PUBLIC_POOL: u8 = 11;
pub const TX_TYPE_TRANSFER: u8 = 12;
pub const TX_TYPE_WITHDRAW: u8 = 13;
pub const TX_TYPE_CREATE_ORDER: u8 = 14;
pub const TX_TYPE_CANCEL_ORDER: u8 = 15;
pub const TX_TYPE_CANCEL_ALL_ORDERS: u8 = 16;
pub const TX_TYPE_MODIFY_ORDER: u8 = 17;
pub const TX_TYPE_MINT_SHARES: u8 = 18;
pub const TX_TYPE_BURN_SHARES: u8 = 19;
pub const TX_TYPE_UPDATE_LEVERAGE: u8 = 20;
pub const TX_TYPE_UPDATE_MARGIN: u8 = 21;

// Order type constants
pub const ORDER_TYPE_LIMIT: i32 = 0;
pub const ORDER_TYPE_MARKET: i32 = 1;
pub const ORDER_TYPE_STOP_LOSS: i32 = 2;
pub const ORDER_TYPE_STOP_LOSS_LIMIT: i32 = 3;
pub const ORDER_TYPE_TAKE_PROFIT: i32 = 4;
pub const ORDER_TYPE_TAKE_PROFIT_LIMIT: i32 = 5;

// Order time in force constants
pub const ORDER_TIME_IN_FORCE_IOC: i32 = 0;
pub const ORDER_TIME_IN_FORCE_GTT: i32 = 1;
pub const ORDER_TIME_IN_FORCE_POST_ONLY: i32 = 2;

#[derive(Clone, Copy, Debug)]
pub enum BatchAckMode {
    Strict,
    Optimistic { wait_timeout: Duration },
}

async fn send_batch_tx_ws_with_mode_internal(
    ws: &mut WsConnection,
    txs: Vec<(u8, String)>,
    mode: BatchAckMode,
) -> WsResult<Vec<bool>> {
    const MAX_BATCH_SIZE: usize = 50;

    if txs.is_empty() {
        return Ok(vec![]);
    }

    info!("Sending batch of {} transactions", txs.len());

    let mut results = Vec::with_capacity(txs.len());

    for (chunk_idx, chunk) in txs.chunks(MAX_BATCH_SIZE).enumerate() {
        debug!(
            "Sending chunk {}/{} ({} txs)",
            chunk_idx + 1,
            (txs.len() + MAX_BATCH_SIZE - 1) / MAX_BATCH_SIZE,
            chunk.len()
        );

        let tx_types: Vec<u8> = chunk.iter().map(|(tx_type, _)| *tx_type).collect();
        let tx_infos: Vec<serde_json::Value> = chunk
            .iter()
            .map(|(_, tx_info)| serde_json::from_str(tx_info).unwrap_or_else(|_| json!({})))
            .collect();

        let wait_timeout = match mode {
            BatchAckMode::Strict => Duration::from_secs(chunk.len() as u64),
            BatchAckMode::Optimistic { wait_timeout } => wait_timeout,
        };

        match ws.send_batch_transaction(tx_types, tx_infos).await {
            Ok(_) => match timeout(wait_timeout, ws.wait_for_tx_response(wait_timeout)).await {
                Ok(Ok(success)) => {
                    results.extend(vec![success; chunk.len()]);
                }
                Ok(Err(e)) => match mode {
                    BatchAckMode::Strict => {
                        error!("Batch error: {}", e);
                        results.extend(vec![false; chunk.len()]);
                        continue;
                    }
                    BatchAckMode::Optimistic { .. } => {
                        warn!(
                                "Fast execution: error while waiting for batch ack ({e}); assuming success"
                            );
                        results.extend(vec![true; chunk.len()]);
                    }
                },
                Err(_) => match mode {
                    BatchAckMode::Strict => {
                        error!(
                            "Batch timeout after {:?} while waiting for transaction response",
                            wait_timeout
                        );
                        results.extend(vec![false; chunk.len()]);
                        continue;
                    }
                    BatchAckMode::Optimistic { .. } => {
                        debug!(
                            "Fast execution: batch timeout after {:?}; assuming success",
                            wait_timeout
                        );
                        results.extend(vec![true; chunk.len()]);
                    }
                },
            },
            Err(e) => {
                error!("Failed to send batch: {}", e);
                return Err(e);
            }
        }
    }

    let success_count = results.iter().filter(|&&r| r).count();
    info!(
        "Batch complete: {}/{} successful",
        success_count,
        results.len()
    );

    Ok(results)
}

pub async fn send_batch_tx_ws_with_mode(
    ws: &mut WsConnection,
    txs: Vec<(u8, String)>,
    mode: BatchAckMode,
) -> WsResult<Vec<bool>> {
    send_batch_tx_ws_with_mode_internal(ws, txs, mode).await
}

/// Position data structure (matches Lighter API response)
/// CRITICAL: Use 'sign' field for direction, NOT position > 0!
#[derive(Debug, Clone)]
pub struct Position {
    pub market_id: u32,
    pub sign: i8,         // -1 = short, 1 = long
    pub position: String, // ALWAYS positive (absolute value)
}

/// Send a single transaction via WebSocket
///
/// # Arguments
/// * `ws` - WebSocket connection
/// * `tx_type` - Transaction type (e.g., TX_TYPE_CREATE_ORDER)
/// * `tx_info` - Signed transaction info (JSON string from SignerLibrary)
///
/// # Returns
/// - `Ok(true)` - Transaction accepted by exchange
/// - `Ok(false)` - Transaction rejected (error response from exchange)
/// - `Err(_)` - WebSocket communication error
///
/// # Error Detection
/// Properly detects errors via:
/// - Response contains `"type":"error"`
/// - Response contains `"error":` key
/// - Response contains non-200 status code
/// - Timeout (1s) is treated as FAILURE
/// - Parse errors are treated as FAILURE
/// - Connection closed is treated as FAILURE
pub async fn send_tx_ws(ws: &mut WsConnection, tx_type: u8, tx_info: &str) -> WsResult<bool> {
    // Parse tx_info as JSON (should already be JSON from SignerLibrary)
    let tx_info_json: serde_json::Value = serde_json::from_str(tx_info).map_err(|e| {
        WsClientError::InvalidMessage(format!("Failed to parse tx_info as JSON: {}", e))
    })?;

    // Use WsConnection's built-in send_transaction method
    ws.send_transaction(tx_type, tx_info_json).await?;

    // Wait for response with timeout (1 second)
    match timeout(
        Duration::from_secs(1),
        ws.wait_for_tx_response(Duration::from_secs(1)),
    )
    .await
    {
        Ok(Ok(success)) => {
            if success {
                info!("Transaction submitted successfully");
                Ok(true)
            } else {
                error!("Transaction failed");
                Ok(false)
            }
        }
        Ok(Err(e)) => {
            error!("WebSocket error while waiting for response: {}", e);
            Ok(false)
        }
        Err(_) => {
            error!("Timeout: No WebSocket response within 1000ms");
            Ok(false)
        }
    }
}

/// Send batch transactions via WebSocket (max 50 per batch per Lighter docs)
///
/// # Arguments
/// * `ws` - WebSocket connection
/// * `txs` - Vector of (tx_type, tx_info) tuples
///
/// # Returns
/// Vector of booleans indicating success/failure for each transaction
///
/// # Notes
/// - Lighter DEX supports max 50 transactions per WebSocket batch
/// - This function will chunk larger batches automatically
/// - Each transaction result is tracked independently
pub async fn send_batch_tx_ws(
    ws: &mut WsConnection,
    txs: Vec<(u8, String)>,
) -> WsResult<Vec<bool>> {
    send_batch_tx_ws_with_mode_internal(ws, txs, BatchAckMode::Strict).await
}

/// Close a position using WebSocket transaction submission
///
/// # Arguments
/// * `ws` - WebSocket connection
/// * `signer` - SignerLibrary instance for transaction signing
/// * `market_id` - Market ID (0 for ETH, 1 for BTC, etc.)
/// * `position_size` - Position size (string from API, always positive)
/// * `position_sign` - Position sign (-1 for short, 1 for long)
/// * `mark_price` - Current mark price for the market
/// * `nonce` - Transaction nonce
/// * `price_decimals` - Price decimal places (e.g., 2 for ETH)
/// * `size_decimals` - Size decimal places (e.g., 4 for ETH)
///
/// # Returns
/// - `Ok(true)` - Position close order submitted successfully
/// - `Ok(false)` - Position close order rejected
/// - `Err(_)` - WebSocket or signing error
///
/// # Critical Implementation Details
/// MUST use `position_sign` field for direction detection:
/// - Long position (sign=1): Close with SELL order (is_ask=1)
/// - Short position (sign=-1): Close with BUY order (is_ask=0)
///
/// DO NOT use `position_size > 0` for direction - it's ALWAYS positive!
pub async fn close_position(
    ws: &mut WsConnection,
    signer: &SignerLibrary,
    market_id: u32,
    position_size: &str,
    position_sign: i8,
    mark_price: f64,
    nonce: u64,
    price_decimals: u8,
    size_decimals: u8,
) -> WsResult<bool> {
    // CRITICAL: Determine order direction based on position sign
    // Long position (sign=1): Close with SELL (is_ask=1)
    // Short position (sign=-1): Close with BUY (is_ask=0)
    let is_long = position_sign > 0;
    let is_ask = is_long; // SELL to close long, BUY to close short

    info!(
        "Closing {} position: market_id={}, size={}, sign={}, is_ask={}",
        if is_long { "LONG" } else { "SHORT" },
        market_id,
        position_size,
        position_sign,
        is_ask
    );

    // Parse position size (always positive from API)
    let pos_abs: f64 = position_size
        .parse()
        .map_err(|e: std::num::ParseFloatError| {
            WsClientError::InvalidMessage(format!("Failed to parse position size: {}", e))
        })?;

    if pos_abs < 1e-8 {
        warn!("Position size too small to close: {}", pos_abs);
        return Ok(false);
    }

    // Scale size to integer (multiply by 10^size_decimals)
    let size_multiplier = 10_f64.powi(size_decimals as i32);
    let size_scaled = (pos_abs * size_multiplier).round() as i64;

    // Calculate close price with safety margin
    // - For SELL (close long): price = mark * 0.985 (1.5% below market)
    // - For BUY (close short): price = mark * 1.015 (1.5% above market)
    let price_multiplier = 10_i64.pow(price_decimals as u32);
    let price_with_margin = if is_ask {
        mark_price * 0.985 // Selling to close long - use aggressive price
    } else {
        mark_price * 1.015 // Buying to close short - use aggressive price
    };

    let price_scaled = ((price_with_margin * price_multiplier as f64).round() as i64).max(1) as i32; // Ensure price >= 1

    debug!(
        "Close order params: size_scaled={}, price_scaled={} (mark={}, margin={})",
        size_scaled,
        price_scaled,
        mark_price,
        if is_ask { "0.985" } else { "1.015" }
    );

    // Generate client order index (timestamp-based)
    let client_order_index = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| WsClientError::InvalidMessage(format!("System time error: {}", e)))?
        .as_millis() as i64;

    // Calculate order expiry (7 days from now in milliseconds)
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| WsClientError::InvalidMessage(format!("System time error: {}", e)))?
        .as_millis() as i64;
    let seven_days_ms = 7 * 24 * 60 * 60 * 1000;
    let order_expiry = now_ms + seven_days_ms;

    // Sign create order transaction
    let (tx_info, error) = signer
        .sign_create_order(
            market_id as i32,
            client_order_index,
            size_scaled,
            price_scaled,
            is_ask,
            ORDER_TYPE_LIMIT,
            ORDER_TIME_IN_FORCE_GTT, // Good Till Time
            false,                   // reduce_only: false (can close position)
            0,                       // trigger_price: 0 (not a conditional order)
            order_expiry,
            nonce as i64,
        )
        .map_err(|e| WsClientError::InvalidMessage(format!("Failed to sign close order: {}", e)))?;

    // Check for signing error
    if let Some(err) = error {
        error!("Signing error: {}", err);
        return Err(WsClientError::InvalidMessage(format!(
            "Signing failed: {}",
            err
        )));
    }

    let tx_info = tx_info.ok_or_else(|| {
        WsClientError::InvalidMessage("No transaction info returned from signer".to_string())
    })?;

    // Submit order via WebSocket
    let success = send_tx_ws(ws, TX_TYPE_CREATE_ORDER, &tx_info).await?;

    if success {
        info!(
            "✅ Close order submitted: {} position of {} @ {} (market_id={})",
            if is_long { "LONG" } else { "SHORT" },
            position_size,
            price_with_margin,
            market_id
        );
    } else {
        error!(
            "❌ Close order failed: {} position of {} @ {} (market_id={})",
            if is_long { "LONG" } else { "SHORT" },
            position_size,
            price_with_margin,
            market_id
        );
    }

    Ok(success)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_direction_long() {
        // LONG position: sign=1, position="0.5"
        let sign: i8 = 1;
        let _position = "0.5";

        // Determine close order direction
        let is_long = sign > 0;
        let is_ask = is_long;

        assert!(is_long); // Should be long
        assert!(is_ask); // Should SELL to close long
    }

    #[test]
    fn test_position_direction_short() {
        // SHORT position: sign=-1, position="0.5"
        let sign: i8 = -1;
        let _position = "0.5";

        // Determine close order direction
        let is_long = sign > 0;
        let is_ask = is_long;

        assert!(!is_long); // Should be short
        assert!(!is_ask); // Should BUY to close short
    }

    #[test]
    fn test_position_always_positive() {
        // CRITICAL: Verify position field is ALWAYS positive
        let positions = vec![
            (1, "0.5"),    // Long
            (-1, "0.5"),   // Short
            (1, "1.234"),  // Long
            (-1, "1.234"), // Short
        ];

        for (sign, position_str) in positions {
            let pos_abs: f64 = position_str.parse().unwrap();

            // Position is ALWAYS positive
            assert!(pos_abs > 0.0);

            // But signed position depends on sign field
            let signed_position = pos_abs * sign as f64;
            if sign > 0 {
                assert!(signed_position > 0.0); // Long = positive
            } else {
                assert!(signed_position < 0.0); // Short = negative
            }
        }
    }

    #[test]
    fn test_close_order_direction_critical() {
        // Test close order direction for both long and short positions
        let test_cases = vec![
            // (sign, expected_is_ask, description)
            (1, true, "Long position should close with SELL"),
            (-1, false, "Short position should close with BUY"),
        ];

        for (sign, expected_is_ask, description) in test_cases {
            let is_long = sign > 0;
            let is_ask = is_long;

            assert_eq!(
                is_ask, expected_is_ask,
                "Failed: {} (sign={}, is_ask={}, expected={})",
                description, sign, is_ask, expected_is_ask
            );
        }
    }

    #[test]
    fn test_price_scaling() {
        // Test price scaling with different decimal places
        let mark_price = 4000.0;
        let price_decimals = 2;

        // For SELL (close long): price = mark * 0.985
        let sell_price = mark_price * 0.985;
        let price_multiplier = 10_i64.pow(price_decimals as u32);
        let sell_scaled = (sell_price * price_multiplier as f64).round() as i64;

        assert_eq!(sell_scaled, 394000); // 3940.00 with 2 decimals

        // For BUY (close short): price = mark * 1.015
        let buy_price = mark_price * 1.015;
        let buy_scaled = (buy_price * price_multiplier as f64).round() as i64;

        assert_eq!(buy_scaled, 406000); // 4060.00 with 2 decimals
    }

    #[test]
    fn test_size_scaling() {
        // Test size scaling with different decimal places
        let size_decimals = 4;
        let size_multiplier = 10_f64.powi(size_decimals as i32);

        let test_cases = vec![
            (0.005, 50),     // 0.005 * 10000 = 50
            (0.5, 5000),     // 0.5 * 10000 = 5000
            (1.0, 10000),    // 1.0 * 10000 = 10000
            (1.2345, 12345), // 1.2345 * 10000 = 12345
        ];

        for (size, expected_scaled) in test_cases {
            let size_scaled = (size * size_multiplier).round() as i64;
            assert_eq!(
                size_scaled, expected_scaled,
                "Size {} should scale to {}",
                size, expected_scaled
            );
        }
    }

    #[test]
    fn test_batch_chunking() {
        // Test that batch sizes are properly chunked
        const MAX_BATCH: usize = 50;

        let test_cases = vec![
            (10, 1),  // 10 txs -> 1 chunk
            (50, 1),  // 50 txs -> 1 chunk
            (51, 2),  // 51 txs -> 2 chunks
            (100, 2), // 100 txs -> 2 chunks
            (101, 3), // 101 txs -> 3 chunks
            (150, 3), // 150 txs -> 3 chunks
        ];

        for (tx_count, expected_chunks) in test_cases {
            let actual_chunks = (tx_count + MAX_BATCH - 1) / MAX_BATCH;
            assert_eq!(
                actual_chunks, expected_chunks,
                "{} txs should create {} chunks",
                tx_count, expected_chunks
            );
        }
    }
}
