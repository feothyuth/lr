//! Batch 20-order BTC grid, then cancel and replace via a second batch.
//!
//! This example:
//! - Builds a symmetric grid of 10 bids and 10 asks around the current BTC mid
//! - Signs everything locally and submits the grid as a single WebSocket batch
//! - Issues a cancel-all followed by an updated grid, again in one batch
//! - Prints basic WebSocket feedback so you can verify acknowledgements
//!
//! Run with the usual `LIGHTER_*` environment variables (see `GUIDE.md`).

#[path = "../common/example_context.rs"]
mod common;

use anyhow::{anyhow, Context, Result};
use common::{connect_private_stream, display_ws_event, sign_cancel_all_for_ws, ExampleContext};
use futures_util::StreamExt;
use lighter_client::{
    lighter_client::LighterClient,
    signer_client::{SignedPayload, SignerClient},
    trading_helpers::{calculate_grid_levels, scale_price_to_int, scale_size_to_int},
    transactions::CreateOrder,
    tx_executor::send_batch_tx_ws,
    types::{BaseQty, MarketId},
    ws_client::WsStream,
};
use std::{
    convert::TryFrom,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::{sleep, timeout};

const DEFAULT_GRID_LEVELS_PER_SIDE: usize = 10;
const GRID_SPACING_PCT: f64 = 0.0002; // 2 bps between levels keeps quotes hugging mid
const GRID_ORDER_SIZE: f64 = 0.0002; // 0.0002 BTC per level as requested
const DEFAULT_EXPIRY: i64 = -1; // Let the gateway fill in the long expiry sentinel
const BOOK_DEPTH: i64 = 50;
const WS_EVENT_TIMEOUT_MS: u64 = 600;
const POST_BATCH_PAUSE_MS: u64 = 300;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("⚡ Grid Batch Example: 20 orders (BTC) + cancel/replace");
    println!("{}", "═".repeat(80));

    let ctx = ExampleContext::initialise(Some("grid_batch_cancel_replace")).await?;
    let client = ctx.client();
    let signer = ctx.signer()?;
    let market = ctx.market_id();
    let market_index: i32 = market.into();

    if market_index != 1 {
        println!(
            "⚠️  This script is tuned for BTC (market_id = 1). Current market_id = {}.",
            market_index
        );
        println!("    Update LIGHTER_MARKET_ID if needed.\n");
    }

    // Fetch lot/price metadata so we scale floats correctly.
    let metadata = client
        .orders()
        .book_details(Some(market))
        .await
        .context("failed to load market metadata")?;
    let detail = metadata
        .order_book_details
        .into_iter()
        .next()
        .context("missing order book details for market")?;

    let symbol = detail.symbol.clone();
    let price_decimals = detail.price_decimals as u8;
    let size_decimals = detail.size_decimals as u8;
    let fallback_price = detail.last_trade_price;
    let grid_levels = grid_levels_per_side();

    let size_units = scale_size_to_int(GRID_ORDER_SIZE, size_decimals);
    let base_amount = BaseQty::try_from(size_units)
        .map_err(|err| anyhow!("invalid grid size {} BTC: {}", GRID_ORDER_SIZE, err))?
        .into_inner();

    println!("📊 Market: {} (ID {})", symbol, market_index);
    println!(
        "   Price decimals: {} | Size decimals: {}",
        price_decimals, size_decimals
    );
    println!(
        "   Lot size chosen: {} ({} scaled units)",
        GRID_ORDER_SIZE, size_units
    );
    println!(
        "   Grid depth: {} levels per side ({} total orders)",
        grid_levels,
        grid_levels * 2
    );

    let mid_price = fetch_mid_price(client, market, fallback_price).await?;
    println!("   Mid reference: ${:.2}", mid_price);

    let (initial_bid_prices, initial_ask_prices) =
        calculate_grid_levels(mid_price, grid_levels, GRID_SPACING_PCT);

    println!(
        "\n🧱 Initial grid ({} bids / {} asks):",
        grid_levels, grid_levels
    );
    print_side("BID", &initial_bid_prices);
    print_side("ASK", &initial_ask_prices);

    let base_index = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;

    let mut initial_orders = sign_side_orders(
        signer,
        market_index,
        base_index,
        &initial_bid_prices,
        base_amount,
        price_decimals,
        false,
    )
    .await?;
    initial_orders.extend(
        sign_side_orders(
            signer,
            market_index,
            base_index + grid_levels as i64,
            &initial_ask_prices,
            base_amount,
            price_decimals,
            true,
        )
        .await?,
    );

    let builder = client
        .ws()
        .subscribe_transactions()
        .subscribe_executed_transactions();
    let (mut stream, _) = connect_private_stream(&ctx, builder).await?;

    println!(
        "\n📤 Sending initial batch ({} orders)...",
        initial_orders.len()
    );
    let initial_payloads = batch_payloads(&initial_orders);
    let initial_results = send_batch_tx_ws(stream.connection_mut(), initial_payloads).await?;
    report_batch("Initial grid submission", &initial_results, None);
    maybe_log_ws_event(&mut stream).await;

    sleep(Duration::from_millis(POST_BATCH_PAUSE_MS)).await;

    // Fetch a fresh mid and shift slightly so the replacement grid visibly changes.
    let refreshed_mid = fetch_mid_price(client, market, mid_price).await?;
    let nudged_mid = refreshed_mid * (1.0 + GRID_SPACING_PCT / 2.0);
    let (replacement_bid_prices, replacement_ask_prices) =
        calculate_grid_levels(nudged_mid, grid_levels, GRID_SPACING_PCT);

    println!(
        "\n♻️  Replacement grid around ${:.2} (shifted {:.2} bps):",
        nudged_mid,
        (GRID_SPACING_PCT / 2.0) * 10_000.0
    );
    print_side("BID", &replacement_bid_prices);
    print_side("ASK", &replacement_ask_prices);

    // Sign cancel-all first so its nonce precedes the replacement orders in the batch.
    let cancel_payload = sign_cancel_all_for_ws(&ctx, 0, 0).await?;

    let mut replacement_orders = sign_side_orders(
        signer,
        market_index,
        base_index + 2 * grid_levels as i64,
        &replacement_bid_prices,
        base_amount,
        price_decimals,
        false,
    )
    .await?;
    replacement_orders.extend(
        sign_side_orders(
            signer,
            market_index,
            base_index + 3 * grid_levels as i64,
            &replacement_ask_prices,
            base_amount,
            price_decimals,
            true,
        )
        .await?,
    );

    let mut replacement_payloads = Vec::with_capacity(1 + replacement_orders.len());
    replacement_payloads.push(cancel_payload);
    replacement_payloads.extend(batch_payloads(&replacement_orders));

    println!(
        "\n📤 Sending cancel-all + {} replacement orders in one batch...",
        replacement_orders.len()
    );
    let replacement_results =
        send_batch_tx_ws(stream.connection_mut(), replacement_payloads).await?;
    report_batch(
        "Cancel + replace batch",
        &replacement_results,
        Some("CancelAll"),
    );
    maybe_log_ws_event(&mut stream).await;

    println!("\n✅ Done. Orders are live until they fill or you cancel them.");
    println!("   Re-run this example to refresh the grid whenever needed.");
    println!("{}", "═".repeat(80));

    Ok(())
}

async fn fetch_mid_price(client: &LighterClient, market: MarketId, fallback: f64) -> Result<f64> {
    let order_book = client.orders().book(market, BOOK_DEPTH).await?;
    let best_bid = order_book
        .bids
        .first()
        .and_then(|bid| bid.price.parse::<f64>().ok());
    let best_ask = order_book
        .asks
        .first()
        .and_then(|ask| ask.price.parse::<f64>().ok());

    Ok(match (best_bid, best_ask) {
        (Some(bid), Some(ask)) if bid > 0.0 && ask > 0.0 => (bid + ask) / 2.0,
        _ => fallback,
    })
}

async fn sign_side_orders(
    signer: &SignerClient,
    market_index: i32,
    client_index_start: i64,
    prices: &[f64],
    base_amount: i64,
    price_decimals: u8,
    is_ask: bool,
) -> Result<Vec<SignedPayload<CreateOrder>>> {
    let mut signed = Vec::with_capacity(prices.len());
    for (offset, price) in prices.iter().enumerate() {
        let ticks = scale_price_to_int(*price, price_decimals);
        let price_i32 =
            i32::try_from(ticks).map_err(|_| anyhow!("price ticks exceed i32 range"))?;
        let (api_key_idx, nonce) = signer.next_nonce().await?;
        let signed_payload = signer
            .sign_create_order(
                market_index,
                client_index_start + offset as i64,
                base_amount,
                price_i32,
                is_ask,
                signer.order_type_limit(),
                signer.order_time_in_force_post_only(),
                false,
                0,
                DEFAULT_EXPIRY,
                Some(nonce),
                Some(api_key_idx),
            )
            .await?;
        signed.push(signed_payload);
    }
    Ok(signed)
}

fn batch_payloads(orders: &[SignedPayload<CreateOrder>]) -> Vec<(u8, String)> {
    orders
        .iter()
        .map(|signed| (signed.tx_type() as u8, signed.payload().to_string()))
        .collect()
}

fn print_side(label: &str, prices: &[f64]) {
    for (idx, price) in prices.iter().enumerate() {
        println!(
            "  {:>3}. {:>3} {:.6} BTC @ ${:.2}",
            idx + 1,
            label,
            GRID_ORDER_SIZE,
            price
        );
    }
}

fn report_batch(label: &str, results: &[bool], first_label: Option<&str>) {
    let successes = results.iter().filter(|&&ok| ok).count();
    println!("   {}: {}/{} accepted", label, successes, results.len());

    for (idx, ok) in results.iter().enumerate() {
        let descriptor = if idx == 0 {
            first_label.unwrap_or("Order")
        } else {
            "Order"
        };
        let status = if *ok { "✅ ACCEPTED" } else { "❌ REJECTED" };
        println!("     {:>2}. {:<10} {}", idx + 1, descriptor, status);
    }
}

async fn maybe_log_ws_event(stream: &mut WsStream) {
    match timeout(Duration::from_millis(WS_EVENT_TIMEOUT_MS), stream.next()).await {
        Ok(Some(Ok(event))) => display_ws_event(event),
        Ok(Some(Err(err))) => println!("⚠️  WebSocket error: {}", err),
        Ok(None) => println!("ℹ️  WebSocket closed"),
        Err(_) => println!("⏱️  No WebSocket events within {} ms", WS_EVENT_TIMEOUT_MS),
    }
}

fn grid_levels_per_side() -> usize {
    std::env::var("GRID_LEVELS_PER_SIDE")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|value| *value >= 1)
        .unwrap_or(DEFAULT_GRID_LEVELS_PER_SIDE)
}
