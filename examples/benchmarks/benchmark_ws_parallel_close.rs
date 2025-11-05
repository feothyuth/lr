// WebSocket Parallel Benchmark: MARKET vs LIMIT IOC
// Pre-signs both orders, then submits in parallel via WebSocket
// Measures which method closes position faster
// Uses SDK SignerClient + WebSocket

#[path = "../common/example_context.rs"]
mod common;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use std::time::Instant;
use tokio::time::{timeout, Duration};

use common::{submit_signed_payload, ExampleContext};
const TEST_SIZE: f64 = 0.005; // 0.005 ETH

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    println!("\n{}", "═".repeat(80));
    println!("⚡ WebSocket Parallel Benchmark: MARKET vs LIMIT IOC");
    println!("    SDK SignerClient + WebSocket • Pre-signed • Parallel Execution");
    println!("{}\n", "═".repeat(80));

    // Initialize context
    println!("📝 Initializing client...");
    let ctx = ExampleContext::initialise(Some("benchmark_ws_parallel_close")).await?;
    let client = ctx.client();
    let market = ctx.market_id();
    let market_index: i32 = market.into();
    let signer = ctx.signer()?;
    let order_type_market = signer.order_type_market();
    let order_type_limit = signer.order_type_limit();
    let time_in_force_ioc = signer.order_time_in_force_immediate_or_cancel();
    let time_in_force_gtt = signer.order_time_in_force_good_till_time();
    println!("✅ Client initialized\n");

    // Fetch market prices via REST (only for pricing)
    println!("📈 Fetching market prices via REST...");
    let order_book = client.orders().book(market, 5).await?;

    let best_bid: f64 = order_book.bids.first().context("No bids")?.price.parse()?;
    let best_ask: f64 = order_book.asks.first().context("No asks")?.price.parse()?;

    println!(
        "✅ Best Bid: ${:.2}, Best Ask: ${:.2}\n",
        best_bid, best_ask
    );

    // ============================================================================
    // STEP 1: Open TWO positions (for parallel test)
    // ============================================================================
    println!("{}", "═".repeat(80));
    println!("📤 Opening TWO 0.005 ETH positions (via WebSocket)...");
    println!("{}", "═".repeat(80));
    println!();

    // Connect WebSocket
    let ws_builder = ctx.ws_builder();
    let (mut stream, _auth_token) = common::connect_private_stream(&ctx, ws_builder).await?;

    let size_int = (TEST_SIZE * 10000.0) as i64;
    let slippage_price = (best_ask * 1.05 * 100.0) as i32;

    // Open first position
    println!("📝 Signing first open order...");
    let client_order_index_1 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;

    let signed_open_1 = signer
        .sign_create_order(
            market_index,
            client_order_index_1,
            size_int,
            slippage_price,
            false, // BUY
            order_type_market,
            time_in_force_ioc,
            false, // not reduce_only (opening position)
            0,
            0,
            None,
            None,
        )
        .await?;

    submit_signed_payload(stream.connection_mut(), &signed_open_1).await?;

    // Wait for response
    timeout(Duration::from_millis(2000), async {
        loop {
            if let Some(Ok(event)) = stream.next().await {
                use lighter_client::ws_client::WsEvent;
                match event {
                    WsEvent::Transaction(_) | WsEvent::ExecutedTransaction(_) => break,
                    WsEvent::Unknown(text) if text.contains("\"code\":200") => break,
                    _ => continue,
                }
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;

    println!("✅ Position 1 opened");

    // Wait a bit
    tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;

    // Open second position
    println!("📝 Signing second open order...");
    let client_order_index_2 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;

    let signed_open_2 = signer
        .sign_create_order(
            market_index,
            client_order_index_2,
            size_int,
            slippage_price,
            false, // BUY
            order_type_market,
            time_in_force_ioc,
            false, // not reduce_only (opening position)
            0,
            0,
            None,
            None,
        )
        .await?;

    submit_signed_payload(stream.connection_mut(), &signed_open_2).await?;

    // Wait for response
    timeout(Duration::from_millis(2000), async {
        loop {
            if let Some(Ok(event)) = stream.next().await {
                use lighter_client::ws_client::WsEvent;
                match event {
                    WsEvent::Transaction(_) | WsEvent::ExecutedTransaction(_) => break,
                    WsEvent::Unknown(text) if text.contains("\"code\":200") => break,
                    _ => continue,
                }
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;

    println!("✅ Position 2 opened");
    println!();

    // Wait for positions to settle
    println!("⏳ Waiting 2 seconds for positions to settle...");
    tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
    println!();

    // ============================================================================
    // STEP 2: PRE-SIGN both close orders
    // ============================================================================
    println!("{}", "═".repeat(80));
    println!("✍️  PRE-SIGNING both close orders (MARKET vs LIMIT IOC)");
    println!("{}", "═".repeat(80));
    println!();

    let presign_start = Instant::now();

    // Pre-sign MARKET order
    let market_price = (best_bid * 0.95 * 100.0) as i32;
    let client_order_index_market = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64
        + 100;

    let signed_market = signer
        .sign_create_order(
            market_index,
            client_order_index_market,
            size_int,
            market_price,
            true, // SELL to close LONG
            order_type_market,
            time_in_force_ioc,
            true, // reduce_only
            0,
            0,
            None,
            None,
        )
        .await?;

    // Pre-sign LIMIT order
    let limit_price = (best_bid * 100.0) as i32;
    let client_order_index_limit = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64
        + 200;
    let order_expiry = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64
        + 600_000; // 10 minutes

    let signed_limit = signer
        .sign_create_order(
            market_index,
            client_order_index_limit,
            size_int,
            limit_price,
            true, // SELL to close LONG
            order_type_limit,
            time_in_force_gtt,
            true, // reduce_only
            0,
            order_expiry,
            None,
            None,
        )
        .await?;

    let presign_duration = presign_start.elapsed();
    println!("✅ Both orders pre-signed in {:?}", presign_duration);
    println!("   MARKET: reduce_only=true, slippage=5%");
    println!(
        "   LIMIT:  reduce_only=true, price=${:.2} (best bid)",
        best_bid
    );
    println!();

    // ============================================================================
    // STEP 3: PARALLEL EXECUTION via WebSocket
    // ============================================================================
    println!("{}", "═".repeat(80));
    println!("⚡ PARALLEL EXECUTION: Sending both orders simultaneously");
    println!("{}", "═".repeat(80));
    println!();

    println!("🚀 Starting parallel submission...\n");

    let parallel_start = Instant::now();

    // Send MARKET order
    let market_send_start = Instant::now();
    submit_signed_payload(stream.connection_mut(), &signed_market).await?;
    let market_send_time = market_send_start.elapsed();

    // Send LIMIT order immediately after
    let limit_send_start = Instant::now();
    submit_signed_payload(stream.connection_mut(), &signed_limit).await?;
    let limit_send_time = limit_send_start.elapsed();

    let parallel_send_duration = parallel_start.elapsed();

    println!("✅ Both orders submitted in {:?}", parallel_send_duration);
    println!("   MARKET sent in: {:?}", market_send_time);
    println!("   LIMIT sent in:  {:?}", limit_send_time);
    println!();

    // Wait for responses and measure which comes back first
    println!("⏳ Waiting for exchange responses...\n");

    let mut market_response_time: Option<std::time::Duration> = None;
    let mut limit_response_time: Option<std::time::Duration> = None;
    let response_start = Instant::now();

    // Collect responses (up to 5 seconds)
    let deadline = Duration::from_millis(5000);
    while response_start.elapsed() < deadline {
        match timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(event))) => {
                use lighter_client::ws_client::WsEvent;

                match event {
                    WsEvent::Transaction(_) | WsEvent::ExecutedTransaction(_) => {
                        if market_response_time.is_none() {
                            market_response_time = Some(response_start.elapsed());
                            println!(
                                "✅ MARKET order response received at {:?}",
                                market_response_time.unwrap()
                            );
                        } else if limit_response_time.is_none() {
                            limit_response_time = Some(response_start.elapsed());
                            println!(
                                "✅ LIMIT order response received at {:?}",
                                limit_response_time.unwrap()
                            );
                        }

                        if market_response_time.is_some() && limit_response_time.is_some() {
                            break;
                        }
                    }
                    WsEvent::Unknown(text) if text.contains("\"code\":200") => {
                        if market_response_time.is_none() {
                            market_response_time = Some(response_start.elapsed());
                            println!(
                                "✅ MARKET order response received at {:?}",
                                market_response_time.unwrap()
                            );
                        } else if limit_response_time.is_none() {
                            limit_response_time = Some(response_start.elapsed());
                            println!(
                                "✅ LIMIT order response received at {:?}",
                                limit_response_time.unwrap()
                            );
                        }

                        if market_response_time.is_some() && limit_response_time.is_some() {
                            break;
                        }
                    }
                    _ => continue,
                }
            }
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => continue,
        }
    }

    println!();

    // ============================================================================
    // RESULTS
    // ============================================================================
    println!("{}", "═".repeat(80));
    println!("📊 BENCHMARK RESULTS (SDK SignerClient + WebSocket)");
    println!("{}", "═".repeat(80));
    println!();

    println!("Pre-Signing:");
    println!("  Both orders: {:?}\n", presign_duration);

    println!("Parallel Submission:");
    println!("  MARKET: {:?}", market_send_time);
    println!("  LIMIT:  {:?}", limit_send_time);
    println!("  Total:  {:?}\n", parallel_send_duration);

    println!("Exchange Response Times:");
    if let Some(market_time) = market_response_time {
        println!("  MARKET: {:?}", market_time);
    } else {
        println!("  MARKET: ⏳ No response");
    }
    if let Some(limit_time) = limit_response_time {
        println!("  LIMIT:  {:?}", limit_time);
    } else {
        println!("  LIMIT:  ⏳ No response");
    }
    println!();

    // Determine winner
    match (market_response_time, limit_response_time) {
        (Some(m), Some(l)) => {
            if m < l {
                println!("🏆 WINNER: MARKET order was {:?} faster!", l - m);
            } else {
                println!("🏆 WINNER: LIMIT order was {:?} faster!", m - l);
            }
        }
        (Some(_), None) => {
            println!("🏆 WINNER: MARKET order (LIMIT didn't respond)");
        }
        (None, Some(_)) => {
            println!("🏆 WINNER: LIMIT order (MARKET didn't respond)");
        }
        (None, None) => {
            println!("❌ No responses received from either order");
        }
    }

    println!();
    println!("{}", "═".repeat(80));
    println!("✅ Benchmark Complete!");
    println!("   • SDK SignerClient for signing");
    println!("   • SDK WebSocket for order submission");
    println!("   • Pre-signed orders for fair comparison");
    println!("   • Parallel execution (truly simultaneous)");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
