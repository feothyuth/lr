//! Shared helpers for latency benchmarking examples.
//!
//! Provides common setup utilities to build a market context, normalise order
//! parameters, and access the websocket helpers defined for examples.

#[path = "../common/example_context.rs"]
pub mod example_context;

use anyhow::{anyhow, Context, Result};
use example_context::ExampleContext;
use lighter_client::{
    lighter_client::LighterClient,
    signer_client::{SignedPayload, SignerClient},
    transactions,
    types::{ApiKeyIndex, BaseQty, Expiry, MarketId, Nonce, Price},
};
use std::{env, time::Instant};

pub use example_context::connect_private_stream;

const DEFAULT_SIZE_BASE: f64 = 0.0002;

pub struct OrderBenchSetup {
    pub ctx: ExampleContext,
    pub price_ticks: i64,
    pub base_qty: BaseQty,
    pub price_usd: f64,
    pub size_base: f64,
}

impl OrderBenchSetup {
    pub fn market_id(&self) -> MarketId {
        self.ctx.market_id()
    }

    pub fn client(&self) -> &LighterClient {
        self.ctx.client()
    }

    pub fn signer(&self) -> Result<&SignerClient> {
        self.ctx.signer()
    }
}

pub async fn prepare_bench_setup(example_key: &str) -> Result<OrderBenchSetup> {
    let ctx = ExampleContext::initialise(Some(example_key)).await?;
    let client = ctx.client();
    let market = ctx.market_id();

    let metadata = client
        .orders()
        .book_details(Some(market))
        .await
        .context("failed to load market metadata")?;
    let detail = metadata
        .order_book_details
        .into_iter()
        .find(|d| d.market_id == market.into_inner())
        .context("market metadata missing for requested market")?;

    let price_multiplier: i64 = 10_i64
        .checked_pow(detail.price_decimals as u32)
        .context("price_decimals overflow")?;
    let size_multiplier: i64 = 10_i64
        .checked_pow(detail.size_decimals as u32)
        .context("size_decimals overflow")?;

    let price_override = read_env_f64_optional("LATENCY_PRICE_USD");
    let mut size_base = read_env_f64("LATENCY_SIZE_BASE", DEFAULT_SIZE_BASE);

    let price_usd = match price_override {
        Some(value) => value,
        None => {
            let ob = client
                .orders()
                .book(market, 5)
                .await
                .context("failed to load order book for price inference")?;
            let best_bid = ob
                .bids
                .first()
                .and_then(|bid| bid.price.parse::<f64>().ok())
                .ok_or_else(|| anyhow!("no bids available to derive price"))?;
            best_bid * 0.9
        }
    };

    let price_ticks_raw = (price_usd * price_multiplier as f64).round() as i64;

    let min_base = detail
        .min_base_amount
        .parse::<f64>()
        .unwrap_or(DEFAULT_SIZE_BASE);
    let min_quote = detail.min_quote_amount.parse::<f64>().unwrap_or(0.0);

    if size_base < min_base {
        println!(
            "Requested size {:.6} below min_base {:.6}; using minimum.",
            size_base, min_base
        );
        size_base = min_base;
    }

    if price_usd * size_base < min_quote {
        let adjusted = (min_quote / price_usd).max(size_base);
        println!(
            "Order value {:.6} below min_quote {:.6}; adjusting size to {:.6}.",
            price_usd * size_base,
            min_quote,
            adjusted
        );
        size_base = adjusted;
    }

    let size_raw = (size_base * size_multiplier as f64).round() as i64;

    let base_qty = BaseQty::try_from(size_raw)
        .map_err(|err| anyhow!("size too small for market decimals: {err}"))?;

    Ok(OrderBenchSetup {
        ctx,
        price_ticks: price_ticks_raw,
        base_qty,
        price_usd,
        size_base,
    })
}

pub fn read_env_usize(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

pub fn read_env_f64(key: &str, default: f64) -> f64 {
    read_env_f64_optional(key).unwrap_or(default)
}

pub fn read_env_f64_optional(key: &str) -> Option<f64> {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| *value > 0.0)
}

pub fn read_env_bool(key: &str, default: bool) -> bool {
    env::var(key)
        .ok()
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

pub fn aggregate_stats(stats: &[f64]) -> (f64, f64, f64, f64, f64) {
    if stats.is_empty() {
        return (0.0, 0.0, 0.0, 0.0, 0.0);
    }

    let mut values = stats.to_vec();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let min = *values.first().unwrap();
    let max = *values.last().unwrap();
    let sum: f64 = stats.iter().sum();
    let avg = sum / stats.len() as f64;
    let median = values[values.len() / 2];
    let p95_index = ((values.len() as f64) * 0.95).floor() as usize;
    let p95 = values[p95_index.min(values.len().saturating_sub(1))];
    (min, median, p95, max, avg)
}

pub async fn fetch_next_nonce(client: &LighterClient, api_key: ApiKeyIndex) -> Result<i64> {
    let response = client.account().next_nonce(api_key).await?;
    if response.code != 200 {
        return Err(anyhow!(
            "next_nonce returned code {} ({:?})",
            response.code,
            response.message
        ));
    }
    Ok(response.nonce)
}

pub async fn sign_limit_order(
    client: &LighterClient,
    market: MarketId,
    base_qty: BaseQty,
    price_ticks: i64,
    is_bid: bool,
    api_key: ApiKeyIndex,
    nonce_value: i64,
    expiry: Expiry,
) -> Result<(SignedPayload<transactions::CreateOrder>, f64)> {
    let start = Instant::now();
    let order_builder = if is_bid {
        client.order(market).buy()
    } else {
        client.order(market).sell()
    };
    let signed = order_builder
        .qty(base_qty)
        .limit(Price::ticks(price_ticks))
        .expires_at(expiry)
        .with_api_key(api_key)
        .with_nonce(Nonce::new(nonce_value))
        .sign()
        .await?;
    let signing_ms = start.elapsed().as_secs_f64() * 1_000.0;
    Ok((signed, signing_ms))
}
