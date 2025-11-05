//! WebSocket Stress Test - High Volume Scenarios
//!
//! Tests:
//! 1. Large WebSocket batch (50 orders - max limit)
//! 2. Rapid market orders (10 consecutive orders with timing)
//! 3. Position flipping (LONG -> close -> SHORT -> close) with timing
//!
//! Run with:
//! LIGHTER_PRIVATE_KEY="..." ACCOUNT_INDEX="..." LIGHTER_API_KEY_INDEX="0" \
//! cargo run --example websocket_stress_test

use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    signer_client::SignerClient,
    tx_executor::{send_batch_tx_ws, TX_TYPE_CREATE_ORDER},
    types::{AccountId, ApiKeyIndex, BaseQty, MarketId},
    ws_client::WsEvent,
};
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   WEBSOCKET STRESS TEST - HIGH VOLUME                       ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    // Load configuration
    let private_key = std::env::var("LIGHTER_PRIVATE_KEY").expect("LIGHTER_PRIVATE_KEY not set");
    let account_index: i64 = std::env::var("ACCOUNT_INDEX")
        .expect("ACCOUNT_INDEX not set")
        .parse()?;
    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")
        .unwrap_or_else(|_| "0".to_string())
        .parse()?;

    println!("📋 Configuration:");
    println!("   Account Index: {}", account_index);
    println!("   API Key Index: {}", api_key_index);
    println!("   Market: BTC-PERP (ID: 1)");
    println!();

    // Build client
    let api_url = "https://mainnet.zklighter.elliot.ai";
    let client = LighterClient::builder()
        .api_url(api_url)
        .private_key(private_key.clone())
        .account_index(AccountId::new(account_index))
        .api_key_index(ApiKeyIndex::new(api_key_index))
        .build()
        .await?;

    println!("✅ Client initialized");
    println!();

    let market = MarketId::new(1); // BTC-PERP

    // Get current market price via WebSocket
    println!("📡 Fetching current market price via WebSocket...");
    let mut stream = client.ws().subscribe_market_stats(market).connect().await?;

    let mut mark_price: Option<f64> = None;
    let timeout = tokio::time::Duration::from_secs(10);
    let start = tokio::time::Instant::now();

    while start.elapsed() < timeout && mark_price.is_none() {
        tokio::select! {
            Some(event) = stream.next() => {
                if let Ok(WsEvent::MarketStats(stats)) = event {
                    mark_price = Some(stats.market_stats.mark_price.parse::<f64>()?);
                    break;
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {}
        }
    }

    let mark_price = mark_price.ok_or("Failed to get mark price")?;
    println!("✅ Current mark price: ${}", mark_price);
    println!();

    // =========================================================================
    // TEST 1: Large WebSocket Batch (50 orders - max limit)
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("TEST 1: Large WebSocket Batch (50 orders max)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    let batch_start = Instant::now();

    // Initialize SignerClient for batch
    let signer = SignerClient::new(
        api_url,
        &private_key,
        api_key_index,
        account_index,
        None,
        None,
        lighter_client::nonce_manager::NonceManagerType::Optimistic,
        None,
    )
    .await?;

    let auth_token = signer.create_auth_token_with_expiry(None)?;

    // Reconnect WebSocket for batch
    let client2 = LighterClient::new(api_url).await?;
    let mut stream2 = client2.ws().subscribe_order_book(market).connect().await?;

    stream2.connection_mut().set_auth_token(auth_token.token);

    let buy_price = ((mark_price * 0.95) * 10.0) as i32; // 5% below market
    let sell_price = ((mark_price * 1.05) * 10.0) as i32; // 5% above market
    let size = 20; // 0.0002 BTC

    println!("📝 Creating batch of 50 orders:");
    println!(
        "   25 BUY orders @ ${:.2} (5% below market)",
        mark_price * 0.95
    );
    println!(
        "   25 SELL orders @ ${:.2} (5% above market)",
        mark_price * 1.05
    );
    println!("   Size: {} units (0.0002 BTC each)", size);
    println!();

    let mut batch_txs = Vec::new();

    // Create 25 BUY orders
    println!("📝 Signing 25 BUY orders...");
    for i in 0..25 {
        let (api_key_idx, nonce) = signer.next_nonce().await?;
        let price = buy_price - (i as i32 * 10); // Slight variation
        let signed = signer
            .sign_create_order(
                1, // BTC-PERP market_id
                chrono::Utc::now().timestamp_millis() + i as i64,
                size,
                price,
                false, // is_ask = false (BUY)
                signer.order_type_limit(),
                signer.order_time_in_force_post_only(),
                false, // reduce_only
                0,
                -1,
                Some(nonce),
                Some(api_key_idx),
            )
            .await?;
        batch_txs.push((TX_TYPE_CREATE_ORDER, signed.payload().to_string()));
    }
    println!("   ✅ 25 BUY orders signed");

    // Create 25 SELL orders
    println!("📝 Signing 25 SELL orders...");
    for i in 0..25 {
        let (api_key_idx, nonce) = signer.next_nonce().await?;
        let price = sell_price + (i as i32 * 10); // Slight variation
        let signed = signer
            .sign_create_order(
                1, // BTC-PERP market_id
                chrono::Utc::now().timestamp_millis() + (i + 100) as i64,
                size,
                price,
                true, // is_ask = true (SELL)
                signer.order_type_limit(),
                signer.order_time_in_force_post_only(),
                false, // reduce_only
                0,
                -1,
                Some(nonce),
                Some(api_key_idx),
            )
            .await?;
        batch_txs.push((TX_TYPE_CREATE_ORDER, signed.payload().to_string()));
    }
    println!("   ✅ 25 SELL orders signed");
    println!();

    println!(
        "📡 Submitting batch of {} orders via WebSocket...",
        batch_txs.len()
    );
    let ws_submit_start = Instant::now();
    let batch_results = send_batch_tx_ws(stream2.connection_mut(), batch_txs).await?;
    let ws_submit_elapsed = ws_submit_start.elapsed();

    let batch_elapsed = batch_start.elapsed();
    let success_count = batch_results.iter().filter(|&&r| r).count();

    println!();
    println!("📊 Large Batch Results:");
    println!("   Total orders: {}", batch_results.len());
    println!("   Succeeded: {}", success_count);
    println!("   Failed: {}", batch_results.len() - success_count);
    println!("   Signing time: {:?}", batch_elapsed - ws_submit_elapsed);
    println!("   WebSocket submission time: {:?}", ws_submit_elapsed);
    println!("   Total time: {:?}", batch_elapsed);
    println!(
        "   Throughput: {:.2} orders/sec",
        batch_results.len() as f64 / batch_elapsed.as_secs_f64()
    );
    println!();

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // =========================================================================
    // TEST 2: Rapid Market Orders (10 consecutive orders)
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("TEST 2: Rapid Market Orders (10 consecutive)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    let rapid_start = Instant::now();
    let mut rapid_times = Vec::new();
    let rapid_size = 20; // 0.0002 BTC

    println!("📝 Submitting 10 market orders in succession...");
    println!();

    for i in 0..10 {
        let is_buy = i % 2 == 0;
        let order_start = Instant::now();

        let result = if is_buy {
            client
                .order(market)
                .buy()
                .qty(BaseQty::try_from(rapid_size)?)
                .market()
                .with_slippage(0.05)
                .submit()
                .await
        } else {
            client
                .order(market)
                .sell()
                .qty(BaseQty::try_from(rapid_size)?)
                .market()
                .with_slippage(0.05)
                .submit()
                .await
        };

        let order_elapsed = order_start.elapsed();
        rapid_times.push(order_elapsed);

        match result {
            Ok(submission) => {
                println!(
                    "   ✅ Order {}: {} {:?}",
                    i + 1,
                    if is_buy { "BUY " } else { "SELL" },
                    order_elapsed
                );
                println!("      TX: {}", submission.response().tx_hash);
            }
            Err(e) => {
                println!("   ❌ Order {}: FAILED {:?}", i + 1, order_elapsed);
                println!("      Error: {}", e);
            }
        }

        // Small delay to avoid overwhelming the system
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    let rapid_elapsed = rapid_start.elapsed();

    println!();
    println!("📊 Rapid Market Orders Results:");
    println!("   Total orders: 10");
    println!("   Total time: {:?}", rapid_elapsed);
    println!(
        "   Average latency: {:?}",
        rapid_times.iter().sum::<std::time::Duration>() / rapid_times.len() as u32
    );
    println!("   Min latency: {:?}", rapid_times.iter().min().unwrap());
    println!("   Max latency: {:?}", rapid_times.iter().max().unwrap());
    println!();

    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // =========================================================================
    // TEST 3: Position Flipping (LONG -> close -> SHORT -> close)
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("TEST 3: Position Flipping with Timing");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    let flip_start = Instant::now();
    let flip_size = 20; // 0.0002 BTC

    // Step 1: Enter LONG
    println!("📝 Step 1: Enter LONG position...");
    let long_entry_start = Instant::now();
    let long_entry = client
        .order(market)
        .buy()
        .qty(BaseQty::try_from(flip_size)?)
        .market()
        .with_slippage(0.05)
        .submit()
        .await?;
    let long_entry_elapsed = long_entry_start.elapsed();
    println!(
        "   ✅ LONG entry: {} ({:?})",
        long_entry.response().tx_hash,
        long_entry_elapsed
    );

    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Step 2: Close LONG
    println!("📝 Step 2: Close LONG position...");
    let long_close_start = Instant::now();
    let long_close = client
        .order(market)
        .sell()
        .qty(BaseQty::try_from(flip_size)?)
        .market()
        .with_slippage(0.05)
        .reduce_only()
        .submit()
        .await?;
    let long_close_elapsed = long_close_start.elapsed();
    println!(
        "   ✅ LONG closed: {} ({:?})",
        long_close.response().tx_hash,
        long_close_elapsed
    );

    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Step 3: Enter SHORT
    println!("📝 Step 3: Enter SHORT position...");
    let short_entry_start = Instant::now();
    let short_entry = client
        .order(market)
        .sell()
        .qty(BaseQty::try_from(flip_size)?)
        .market()
        .with_slippage(0.05)
        .submit()
        .await?;
    let short_entry_elapsed = short_entry_start.elapsed();
    println!(
        "   ✅ SHORT entry: {} ({:?})",
        short_entry.response().tx_hash,
        short_entry_elapsed
    );

    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Step 4: Close SHORT
    println!("📝 Step 4: Close SHORT position...");
    let short_close_start = Instant::now();
    let short_close = client
        .order(market)
        .buy()
        .qty(BaseQty::try_from(flip_size)?)
        .market()
        .with_slippage(0.05)
        .reduce_only()
        .submit()
        .await?;
    let short_close_elapsed = short_close_start.elapsed();
    println!(
        "   ✅ SHORT closed: {} ({:?})",
        short_close.response().tx_hash,
        short_close_elapsed
    );

    let flip_elapsed = flip_start.elapsed();

    println!();
    println!("📊 Position Flipping Results:");
    println!("   LONG entry latency: {:?}", long_entry_elapsed);
    println!("   LONG close latency: {:?}", long_close_elapsed);
    println!("   SHORT entry latency: {:?}", short_entry_elapsed);
    println!("   SHORT close latency: {:?}", short_close_elapsed);
    println!(
        "   Average operation latency: {:?}",
        (long_entry_elapsed + long_close_elapsed + short_entry_elapsed + short_close_elapsed) / 4
    );
    println!("   Total cycle time (with delays): {:?}", flip_elapsed);
    println!();

    // =========================================================================
    // FINAL SUMMARY
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📊 STRESS TEST SUMMARY");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("1. Large WebSocket Batch (50 orders):");
    println!(
        "   ✅ {}/{} succeeded in {:?}",
        success_count,
        batch_results.len(),
        batch_elapsed
    );
    println!(
        "   Throughput: {:.2} orders/sec",
        batch_results.len() as f64 / batch_elapsed.as_secs_f64()
    );
    println!();
    println!("2. Rapid Market Orders (10 consecutive):");
    println!("   ✅ Completed in {:?}", rapid_elapsed);
    println!(
        "   Average latency: {:?}",
        rapid_times.iter().sum::<std::time::Duration>() / rapid_times.len() as u32
    );
    println!();
    println!("3. Position Flipping (4 operations):");
    println!("   ✅ Full cycle in {:?}", flip_elapsed);
    println!(
        "   Average per operation: {:?}",
        (long_entry_elapsed + long_close_elapsed + short_entry_elapsed + short_close_elapsed) / 4
    );
    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("✅ WEBSOCKET STRESS TEST COMPLETE");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    Ok(())
}
