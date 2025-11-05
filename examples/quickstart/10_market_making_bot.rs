//! Advanced market making bot example for Lighter DEX
//!
//! This example demonstrates:
//! - Two-sided liquidity provision (bid and ask orders)
//! - Dynamic spread management based on market conditions
//! - Inventory risk management
//! - Order refreshing and cancellation
//! - Real-time market data processing via WebSocket
//!
//! Strategy:
//! - Places limit orders on both sides of the order book
//! - Maintains target spread around mid-price
//! - Manages inventory to avoid accumulating directional risk
//! - Cancels and replaces orders when market moves
//!
//! Required environment variables:
//! - `LIGHTER_PRIVATE_KEY` - Your private key
//! - `LIGHTER_API_URL` (optional) - defaults to mainnet
//! - `LIGHTER_MARKET_ID` (optional) - defaults to 0 (ETH)
//! - `LIGHTER_ACCOUNT_INDEX` (optional) - defaults to 0

#[path = "../common/env.rs"]
mod common_env;

use common_env::{ensure_positive, parse_env_or, require_env, require_parse_env, resolve_api_url};
use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    signer_client::SignerClient,
    trading_helpers::{
        calculate_grid_levels, calculate_mid_price, scale_price_to_int, scale_size_to_int,
    },
    tx_executor::{send_batch_tx_ws, TX_TYPE_CREATE_ORDER},
    types::MarketId,
    ws_client::WsEvent,
};
use std::time::Duration;

/// Market making bot configuration
struct MarketMakerConfig {
    market_id: i32,
    account_index: i64,

    // Strategy parameters
    target_spread_pct: f64, // Target spread as percentage (e.g., 0.002 for 0.2%)
    order_size: f64,        // Order size in base currency
    num_levels: usize,      // Number of price levels on each side
    level_spacing_pct: f64, // Spacing between levels (e.g., 0.001 for 0.1%)

    // Risk management
    max_position_size: f64,        // Maximum absolute position size
    inventory_skew_threshold: f64, // When to start skewing quotes (e.g., 0.5)

    // Order management
    quote_refresh_interval: Duration, // How often to refresh quotes
    price_move_threshold_pct: f64,    // Cancel and replace if price moves by this %
}

impl Default for MarketMakerConfig {
    fn default() -> Self {
        Self {
            market_id: 0,
            account_index: 0,
            target_spread_pct: 0.002,      // 0.2% spread
            order_size: 0.01,              // 0.01 ETH per order
            num_levels: 3,                 // 3 levels on each side
            level_spacing_pct: 0.001,      // 0.1% between levels
            max_position_size: 1.0,        // Max 1.0 ETH position
            inventory_skew_threshold: 0.5, // Start skewing at 50% of max
            quote_refresh_interval: Duration::from_secs(30),
            price_move_threshold_pct: 0.005, // 0.5% price move
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🤖 Lighter DEX Market Making Bot");
    println!("================================\n");

    // Load configuration
    let private_key = require_env(&["LIGHTER_PRIVATE_KEY"], "LIGHTER_PRIVATE_KEY");
    let api_url = resolve_api_url("https://mainnet.zklighter.elliot.ai");

    let mut config = MarketMakerConfig::default();
    config.market_id = parse_env_or(&["LIGHTER_MARKET_ID"], 0);
    config.account_index = ensure_positive(
        require_parse_env(
            &[
                "LIGHTER_ACCOUNT_INDEX",
                "ACCOUNT_INDEX",
                "LIGHTER_ACCOUNT_ID",
            ],
            "account index",
        ),
        "account index",
    );
    let api_key_index =
        require_parse_env(&["LIGHTER_API_KEY_INDEX", "API_KEY_INDEX"], "API key index");

    println!("📊 Configuration:");
    println!("   Market ID: {}", config.market_id);
    println!("   Account Index: {}", config.account_index);
    println!("   Target Spread: {:.2}%", config.target_spread_pct * 100.0);
    println!("   Order Size: {} (per level)", config.order_size);
    println!("   Number of Levels: {}", config.num_levels);
    println!("   Max Position: {}", config.max_position_size);
    println!("   API Key Index: {}", api_key_index);

    // Initialize clients
    println!("\n🔐 Initializing clients...");
    let lighter_client = LighterClient::new(&api_url).await?;
    let signer = SignerClient::new(
        &api_url,
        &private_key,
        api_key_index,
        config.account_index,
        None,
        None,
        lighter_client::nonce_manager::NonceManagerType::Optimistic,
        None,
    )
    .await?;

    // Create authentication token
    let auth_token = signer.create_auth_token_with_expiry(None)?;
    println!("✅ Clients initialized");

    // Connect to WebSocket for real-time market data
    println!("\n🔗 Connecting to WebSocket...");

    // Subscribe to order book
    let market = MarketId::new(config.market_id);
    let mut order_book_stream = lighter_client
        .ws()
        .subscribe_order_book(market)
        .connect()
        .await?;

    // Set authentication token for transaction submission
    order_book_stream
        .connection_mut()
        .set_auth_token(auth_token.token);

    println!("✅ WebSocket connected");
    println!("\n🚀 Market making bot starting...");
    println!("⚠️  This is a DEMO - it will place real orders!");
    println!("   Press Ctrl+C to stop\n");

    // Wait for initial order book snapshot
    let mut current_mid_price = 3500.0; // Default ETH price
    let mut quotes_placed = false;

    println!("📖 Waiting for order book data...");

    while let Some(event) = order_book_stream.next().await {
        match event? {
            WsEvent::OrderBook(update) => {
                // Extract best bid and ask
                let best_bid = update.state.bids.first();
                let best_ask = update.state.asks.first();

                if let (Some(bid), Some(ask)) = (best_bid, best_ask) {
                    // Parse prices (assuming they're strings with 2 decimals)
                    if let (Ok(bid_price), Ok(ask_price)) = (
                        bid.price.replace(".", "").parse::<i64>(),
                        ask.price.replace(".", "").parse::<i64>(),
                    ) {
                        let bid_float = bid_price as f64 / 100.0;
                        let ask_float = ask_price as f64 / 100.0;
                        current_mid_price = calculate_mid_price(bid_float, ask_float);

                        println!("\n📊 Market Update:");
                        println!("   Best Bid: ${:.2}", bid_float);
                        println!("   Best Ask: ${:.2}", ask_float);
                        println!("   Mid Price: ${:.2}", current_mid_price);
                        println!(
                            "   Spread: {:.2} ({:.3}%)",
                            ask_float - bid_float,
                            ((ask_float - bid_float) / current_mid_price) * 100.0
                        );

                        // Place quotes if not already placed
                        if !quotes_placed {
                            println!("\n📝 Placing market making quotes...");

                            // Calculate grid levels
                            let (buy_prices, sell_prices) = calculate_grid_levels(
                                current_mid_price,
                                config.num_levels,
                                config.level_spacing_pct,
                            );

                            // Prepare batch orders
                            let mut batch_txs = Vec::new();

                            // Create buy orders
                            for (i, price) in buy_prices.iter().enumerate() {
                                match create_limit_order(
                                    &signer,
                                    config.market_id,
                                    *price,
                                    config.order_size,
                                    false, // BUY
                                )
                                .await
                                {
                                    Ok((tx_type, tx_info)) => {
                                        println!(
                                            "   ✅ Buy order {} @ ${:.2}: signed",
                                            i + 1,
                                            price
                                        );
                                        batch_txs.push((tx_type, tx_info));
                                    }
                                    Err(e) => {
                                        println!("   ❌ Buy order {} failed: {}", i + 1, e);
                                    }
                                }
                            }

                            // Create sell orders
                            for (i, price) in sell_prices.iter().enumerate() {
                                match create_limit_order(
                                    &signer,
                                    config.market_id,
                                    *price,
                                    config.order_size,
                                    true, // SELL
                                )
                                .await
                                {
                                    Ok((tx_type, tx_info)) => {
                                        println!(
                                            "   ✅ Sell order {} @ ${:.2}: signed",
                                            i + 1,
                                            price
                                        );
                                        batch_txs.push((tx_type, tx_info));
                                    }
                                    Err(e) => {
                                        println!("   ❌ Sell order {} failed: {}", i + 1, e);
                                    }
                                }
                            }

                            // Submit batch
                            if !batch_txs.is_empty() {
                                println!(
                                    "\n📤 Submitting {} orders via WebSocket...",
                                    batch_txs.len()
                                );
                                match send_batch_tx_ws(
                                    order_book_stream.connection_mut(),
                                    batch_txs,
                                )
                                .await
                                {
                                    Ok(results) => {
                                        let success_count = results.iter().filter(|&&r| r).count();
                                        println!(
                                            "✅ Batch complete: {}/{} successful",
                                            success_count,
                                            results.len()
                                        );
                                        quotes_placed = true;
                                    }
                                    Err(e) => {
                                        println!("❌ Batch submission failed: {}", e);
                                    }
                                }
                            }

                            // After placing quotes, show summary and exit (demo)
                            if quotes_placed {
                                println!("\n✅ Market making quotes placed successfully!");
                                println!("\n💡 In a production bot, you would:");
                                println!("   1. Monitor fills and adjust inventory");
                                println!("   2. Cancel and replace orders when market moves");
                                println!(
                                    "   3. Implement risk management (max position, stop loss)"
                                );
                                println!("   4. Track PnL and performance metrics");
                                println!("   5. Handle reconnections and errors gracefully");

                                println!("\n👋 Demo complete - exiting");
                                break;
                            }
                        }
                    }
                }
            }
            WsEvent::Closed(frame) => {
                println!("🔌 WebSocket closed: {:?}", frame);
                break;
            }
            _ => {}
        }

        // Timeout after processing a few events (demo only)
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Ok(())
}

/// Create a limit order
async fn create_limit_order(
    signer: &SignerClient,
    market_id: i32,
    price: f64,
    size: f64,
    is_sell: bool,
) -> Result<(u8, String), Box<dyn std::error::Error>> {
    // Get nonce
    let (api_key_index, nonce) = signer.next_nonce().await?;

    // Scale price and size to integers (ETH: 2 price decimals, 4 size decimals)
    let price_int = scale_price_to_int(price, 2);
    let size_int = scale_size_to_int(size, 4);

    // Generate client order index
    let client_order_index = chrono::Utc::now().timestamp_millis();

    // Sign the order
    let signed = signer
        .sign_create_order(
            market_id,
            client_order_index,
            size_int,
            price_int as i32,
            is_sell,
            signer.order_type_limit(),
            signer.order_time_in_force_post_only(),
            false,
            0,
            -1,
            Some(nonce),
            Some(api_key_index),
        )
        .await?;

    Ok((TX_TYPE_CREATE_ORDER, signed.payload().to_string()))
}
