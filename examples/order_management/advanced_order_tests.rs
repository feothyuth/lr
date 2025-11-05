//! Advanced Order Testing
//!
//! Tests:
//! 1. Limit post-only reduce-only order (will fail - no position, expected)
//! 2. Batch transaction: 3 longs at $100k + 3 shorts at $120k
//! 3. Market entry (0.00001 BTC) + market reduce-only exit
//!
//! Run with:
//! LIGHTER_PRIVATE_KEY="..." ACCOUNT_INDEX="..." LIGHTER_API_KEY_INDEX="0" \
//! cargo run --example advanced_order_tests

use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    signer_client::SignerClient,
    tx_executor::{send_batch_tx_ws, TX_TYPE_CREATE_ORDER},
    types::{AccountId, ApiKeyIndex, BaseQty, Expiry, MarketId, Price},
    ws_client::WsEvent,
};
use time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   ADVANCED ORDER TESTS - LIGHTER DEX MAINNET                 ║");
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

    // Get current market price
    println!("📡 Fetching current market price...");
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

    let mut results = TestResults::new();

    // =========================================================================
    // TEST 1: Limit Post-Only Reduce-Only
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("TEST 1: Limit Post-Only Reduce-Only Order");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("📝 This will fail (expected) - no position to reduce");
    println!();

    let reduce_only_price = (mark_price * 10.0 * 0.9999) as i64;

    println!("📡 Submitting reduce-only order...");
    match client
        .order(market)
        .buy()
        .qty(BaseQty::try_from(20)?)
        .limit(Price::ticks(reduce_only_price))
        .expires_at(Expiry::from_now(Duration::minutes(10)))
        .post_only()
        .reduce_only()
        .submit()
        .await
    {
        Ok(submission) => {
            println!(
                "   ⚠️  Unexpectedly succeeded: {}",
                submission.response().tx_hash
            );
            results.reduce_only_passed = false;
        }
        Err(e) => {
            let err_str = format!("{}", e);
            if err_str.contains("no position") || err_str.contains("reduce") {
                println!("   ✅ Correctly rejected (no position to reduce)");
                results.reduce_only_passed = true;
            } else {
                println!("   ❌ Failed with unexpected error: {}", e);
                results.reduce_only_passed = false;
            }
            results.reduce_only_error = Some(err_str);
        }
    }
    println!();

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // =========================================================================
    // TEST 2: Batch Transaction (3 longs at $100k + 3 shorts at $120k)
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("TEST 2: Batch Transaction (3 Longs + 3 Shorts)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

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

    // Reconnect to WebSocket for batch
    let client2 = LighterClient::new(api_url).await?;
    let mut stream2 = client2.ws().subscribe_order_book(market).connect().await?;

    stream2.connection_mut().set_auth_token(auth_token.token);

    let long_price = 100_000 * 10; // $100k in ticks
    let short_price = 120_000 * 10; // $120k in ticks
    let size = 20; // 0.0002 BTC

    println!("📝 Creating batch of 6 orders:");
    println!("   3 Longs (BUY) at ${} (1,000,000 ticks)", 100_000);
    println!("   3 Shorts (SELL) at ${} (1,200,000 ticks)", 120_000);
    println!("   Size: {} units (0.0002 BTC each)", size);
    println!();

    let mut batch_txs = Vec::new();

    // Create 3 long orders at $100k
    for i in 0..3 {
        let (api_key_idx, nonce) = signer.next_nonce().await?;
        let signed = signer
            .sign_create_order(
                1, // BTC-PERP market_id
                chrono::Utc::now().timestamp_millis() + i as i64,
                size,
                long_price,
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
        println!("   ✅ Signed LONG order {} @ $100,000", i + 1);
    }

    // Create 3 short orders at $120k
    for i in 0..3 {
        let (api_key_idx, nonce) = signer.next_nonce().await?;
        let signed = signer
            .sign_create_order(
                1, // BTC-PERP market_id
                chrono::Utc::now().timestamp_millis() + (i + 100) as i64,
                size,
                short_price,
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
        println!("   ✅ Signed SHORT order {} @ $120,000", i + 1);
    }

    println!();
    println!("📡 Submitting batch of {} transactions...", batch_txs.len());

    let batch_results = send_batch_tx_ws(stream2.connection_mut(), batch_txs).await?;

    let success_count = batch_results.iter().filter(|&&r| r).count();
    results.batch_success_count = success_count;
    results.batch_total_count = batch_results.len();

    println!();
    println!("📊 Batch Results:");
    for (i, success) in batch_results.iter().enumerate() {
        let order_type = if i < 3 { "LONG" } else { "SHORT" };
        let status = if *success { "✅" } else { "❌" };
        println!(
            "   {} {} Order {}: {}",
            status,
            order_type,
            (i % 3) + 1,
            if *success { "SUCCESS" } else { "FAILED" }
        );
    }
    println!();
    println!(
        "   Total: {}/{} succeeded",
        success_count,
        batch_results.len()
    );
    println!();

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // =========================================================================
    // TEST 3: Market Entry + Reduce-Only Exit
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("TEST 3: Market Entry + Reduce-Only Exit");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    let entry_size = 1; // 0.00001 BTC (minimum is 0.0002, so this is 1/20th)

    println!("📝 Step 1: Market entry");
    println!("   Size: {} units (0.00001 BTC)", entry_size);
    println!();

    println!("📡 Submitting market entry order...");
    match client
        .order(market)
        .buy()
        .qty(BaseQty::try_from(entry_size)?)
        .market()
        .submit()
        .await
    {
        Ok(submission) => {
            println!("   ✅ Market entry executed!");
            println!("   TX: {}", submission.response().tx_hash);
            results.market_entry_tx = Some(submission.response().tx_hash.clone());
            results.market_entry_success = true;
        }
        Err(e) => {
            println!("   ❌ Market entry failed: {}", e);
            results.market_entry_success = false;
            results.market_entry_error = Some(format!("{}", e));
        }
    }

    println!();
    println!("⏰ Waiting 3 seconds for position to settle...");
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    if results.market_entry_success {
        println!();
        println!("📝 Step 2: Market reduce-only exit");
        println!("   Size: {} units (close the position)", entry_size);
        println!();

        println!("📡 Submitting market reduce-only exit...");
        match client
            .order(market)
            .sell() // Opposite side to close long position
            .qty(BaseQty::try_from(entry_size)?)
            .market()
            .reduce_only()
            .submit()
            .await
        {
            Ok(submission) => {
                println!("   ✅ Market exit executed!");
                println!("   TX: {}", submission.response().tx_hash);
                results.market_exit_tx = Some(submission.response().tx_hash.clone());
                results.market_exit_success = true;
            }
            Err(e) => {
                println!("   ❌ Market exit failed: {}", e);
                results.market_exit_success = false;
                results.market_exit_error = Some(format!("{}", e));
            }
        }
    } else {
        println!();
        println!("⚠️  Skipping exit test (entry failed)");
    }

    println!();
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // =========================================================================
    // SUMMARY
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📊 TEST RESULTS SUMMARY");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    results.print_summary();

    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("✅ ADVANCED ORDER TESTS COMPLETE");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    Ok(())
}

#[derive(Debug)]
struct TestResults {
    reduce_only_passed: bool,
    reduce_only_error: Option<String>,

    batch_success_count: usize,
    batch_total_count: usize,

    market_entry_success: bool,
    market_entry_tx: Option<String>,
    market_entry_error: Option<String>,

    market_exit_success: bool,
    market_exit_tx: Option<String>,
    market_exit_error: Option<String>,
}

impl TestResults {
    fn new() -> Self {
        Self {
            reduce_only_passed: false,
            reduce_only_error: None,
            batch_success_count: 0,
            batch_total_count: 0,
            market_entry_success: false,
            market_entry_tx: None,
            market_entry_error: None,
            market_exit_success: false,
            market_exit_tx: None,
            market_exit_error: None,
        }
    }

    fn print_summary(&self) {
        println!("1. Reduce-Only Order Test:");
        if self.reduce_only_passed {
            println!("   ✅ PASSED (correctly rejected, no position)");
        } else {
            println!("   ❌ FAILED");
        }
        if let Some(err) = &self.reduce_only_error {
            println!("   Error: {}", &err[..err.len().min(100)]);
        }
        println!();

        println!("2. Batch Transaction Test:");
        println!("   Submitted: {} orders", self.batch_total_count);
        println!("   Succeeded: {} orders", self.batch_success_count);
        println!(
            "   Failed: {} orders",
            self.batch_total_count - self.batch_success_count
        );
        if self.batch_success_count == self.batch_total_count {
            println!("   ✅ ALL SUCCEEDED");
        } else if self.batch_success_count > 0 {
            println!("   ⚠️  PARTIAL SUCCESS");
        } else {
            println!("   ❌ ALL FAILED");
        }
        println!();

        println!("3. Market Entry + Exit Test:");
        println!(
            "   Entry: {}",
            if self.market_entry_success {
                "✅ SUCCESS"
            } else {
                "❌ FAILED"
            }
        );
        if let Some(tx) = &self.market_entry_tx {
            println!("   Entry TX: {}", tx);
        }
        if let Some(err) = &self.market_entry_error {
            println!("   Entry Error: {}", &err[..err.len().min(100)]);
        }

        println!(
            "   Exit: {}",
            if self.market_exit_success {
                "✅ SUCCESS"
            } else {
                "❌ FAILED"
            }
        );
        if let Some(tx) = &self.market_exit_tx {
            println!("   Exit TX: {}", tx);
        }
        if let Some(err) = &self.market_exit_error {
            println!("   Exit Error: {}", &err[..err.len().min(100)]);
        }
        println!();

        let total_tests = 3;
        let passed = (if self.reduce_only_passed { 1 } else { 0 })
            + (if self.batch_success_count > 0 { 1 } else { 0 })
            + (if self.market_entry_success && self.market_exit_success {
                1
            } else {
                0
            });

        println!(
            "Overall: {}/{} major tests had success",
            passed, total_tests
        );
    }
}
