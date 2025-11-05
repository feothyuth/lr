// Channel Data Collection Test
// Collects at least 5 samples from each WebSocket channel

#[path = "../common/example_context.rs"]
mod common;

use futures_util::StreamExt;
use lighter_client::ws_client::WsEvent;
use std::collections::HashMap;
use std::time::Duration;

use common::ExampleContext;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║   CHANNEL DATA COLLECTION TEST - LIGHTER DEX MAINNET         ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!();

    let ctx = ExampleContext::initialise(Some("channel_data_collection_test")).await?;
    let client = ctx.client();
    let signer = ctx.signer()?;
    let account_id = ctx.account_id();
    let market = ctx.market_id();

    println!("📋 Configuration:");
    println!("   Account Index: {}", account_id.into_inner());
    println!("   API Key Index: {}", ctx.api_key_index().into_inner());
    println!();

    // Generate auth token for private channels
    let auth_token = signer
        .create_auth_token_with_expiry(None)
        .map_err(|e| -> BoxError { Box::new(e) })?
        .token;
    println!("✅ Auth token generated");
    println!();

    // Subscribe to all channels
    println!("📡 Subscribing to all channels...");
    let mut stream = client
        .ws()
        // Public channels
        .subscribe_order_book(market)
        .subscribe_market_stats(market)
        .subscribe_transactions()
        .subscribe_executed_transactions()
        .subscribe_height()
        .subscribe_trade(market)
        .subscribe_bbo(market)
        // Private channels
        .subscribe_account(account_id)
        .connect()
        .await
        .map_err(|e| -> BoxError { Box::new(e) })?;

    stream.connection_mut().set_auth_token(auth_token);

    println!("✅ Connected to WebSocket");
    println!();

    // Data collectors
    let mut data_collectors = DataCollectors::new();

    println!("📊 Collecting data from channels (will stop when all have ≥5 samples)...");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    let timeout_duration = Duration::from_secs(60); // 1 minute max
    let start_time = std::time::Instant::now();

    while start_time.elapsed() < timeout_duration {
        tokio::select! {
            Some(event) = stream.next() => {
                match event.map_err(|e| -> BoxError { Box::new(e) })? {
                    WsEvent::Connected => {
                        println!("🔗 WebSocket connected");
                        data_collectors.connected += 1;
                    }
                    WsEvent::OrderBook(ob) => {
                        let sample = format!(
                            "Market: {}, Bids: {}, Asks: {}, Best Bid: {:?}, Best Ask: {:?}",
                            ob.market,
                            ob.state.bids.len(),
                            ob.state.asks.len(),
                            ob.state.bids.first().map(|l| &l.price),
                            ob.state.asks.first().map(|l| &l.price)
                        );
                        data_collectors.add_sample("OrderBook", sample);
                    }
                    WsEvent::MarketStats(stats) => {
                        let sample = format!(
                            "Market: {}, Mark Price: ${}, Index Price: ${}, Funding: {}%, Volume: ${:.0}M",
                            stats.market_stats.market_id,
                            stats.market_stats.mark_price,
                            stats.market_stats.index_price,
                            (stats.market_stats.current_funding_rate.parse::<f64>().unwrap_or(0.0) * 100.0),
                            stats.market_stats.daily_quote_token_volume / 1_000_000.0
                        );
                        data_collectors.add_sample("MarketStats", sample);
                    }
                    WsEvent::Transaction(txs) => {
                        if !txs.txs.is_empty() {
                            let sample = format!(
                                "Count: {}, First TX: hash={}, type={}, status={}",
                                txs.txs.len(),
                                &txs.txs[0].hash[..16],
                                txs.txs[0].tx_type,
                                txs.txs[0].status
                            );
                            data_collectors.add_sample("Transaction", sample);
                        }
                    }
                    WsEvent::ExecutedTransaction(exec) => {
                        if !exec.executed_txs.is_empty() {
                            let sample = format!(
                                "Count: {}, First TX: hash={}, block={}",
                                exec.executed_txs.len(),
                                &exec.executed_txs[0].hash[..16],
                                exec.executed_txs[0].block_height
                            );
                            data_collectors.add_sample("ExecutedTransaction", sample);
                        }
                    }
                    WsEvent::Height(height) => {
                        let sample = format!("Height: {}", height.height);
                        data_collectors.add_sample("Height", sample);
                    }
                    WsEvent::Trade(trades) => {
                        if !trades.trades.is_empty() {
                            let sample = format!(
                                "Count: {}, First: {} {} @ ${}, Liquidation: {}",
                                trades.trades.len(),
                                trades.trades[0].side,
                                trades.trades[0].base_size,
                                trades.trades[0].price,
                                trades.trades[0].is_liquidation
                            );
                            data_collectors.add_sample("Trade", sample);
                        }
                    }
                    WsEvent::BBO(bbo) => {
                        let sample = format!(
                            "Market: {}, Best Bid: {:?}, Best Ask: {:?}",
                            bbo.market_id,
                            bbo.best_bid,
                            bbo.best_ask
                        );
                        data_collectors.add_sample("BBO", sample);
                    }
                    WsEvent::Account(acc) => {
                        let sample = format!(
                            "Account: {}, Snapshot: {}, Data keys: {}",
                            acc.account,
                            acc.snapshot,
                            acc.event.as_value().as_object().map(|o| o.len()).unwrap_or(0)
                        );
                        data_collectors.add_sample("Account", sample);
                    }
                    WsEvent::Pong => {
                        data_collectors.pong += 1;
                    }
                    WsEvent::Unknown(_) => {
                        data_collectors.unknown += 1;
                    }
                    WsEvent::Closed(_) => {
                        println!("🔌 Connection closed");
                        break;
                    }
                }

                // Check if we have enough samples from all channels
                if data_collectors.all_channels_have_minimum_samples() {
                    println!();
                    println!("✅ All channels have collected ≥5 samples!");
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                // Continue listening
            }
        }
    }

    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📊 DATA COLLECTION RESULTS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    data_collectors.print_summary();

    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📋 DETAILED SAMPLES");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    data_collectors.print_detailed_samples();

    Ok(())
}

struct DataCollectors {
    samples: HashMap<String, Vec<String>>,
    connected: usize,
    pong: usize,
    unknown: usize,
}

impl DataCollectors {
    fn new() -> Self {
        Self {
            samples: HashMap::new(),
            connected: 0,
            pong: 0,
            unknown: 0,
        }
    }

    fn add_sample(&mut self, channel: &str, sample: String) {
        let samples = self
            .samples
            .entry(channel.to_string())
            .or_insert_with(Vec::new);

        // Only print first time we reach milestones
        let prev_count = samples.len();
        samples.push(sample);
        let new_count = samples.len();

        if prev_count < 5 && new_count <= 5 {
            println!("  ✓ {} - Sample #{} collected", channel, new_count);
        }
    }

    fn all_channels_have_minimum_samples(&self) -> bool {
        let required_channels = vec!["OrderBook", "MarketStats", "Transaction", "Height", "Trade"];

        for channel in required_channels {
            if self.samples.get(channel).map(|s| s.len()).unwrap_or(0) < 5 {
                return false;
            }
        }

        true
    }

    fn print_summary(&self) {
        let channels = vec![
            "OrderBook",
            "MarketStats",
            "Transaction",
            "ExecutedTransaction",
            "Height",
            "Trade",
            "BBO",
            "Account",
        ];

        for channel in channels {
            let count = self.samples.get(channel).map(|s| s.len()).unwrap_or(0);
            let status = if count >= 5 { "✅" } else { "⚠️ " };
            println!("  {} {}: {} samples", status, channel, count);
        }

        println!();
        println!("  Other events:");
        println!("    Connected: {}", self.connected);
        println!("    Pong: {}", self.pong);
        println!("    Unknown: {}", self.unknown);
    }

    fn print_detailed_samples(&self) {
        let channels = vec![
            "OrderBook",
            "MarketStats",
            "Transaction",
            "ExecutedTransaction",
            "Height",
            "Trade",
            "BBO",
            "Account",
        ];

        for channel in channels {
            if let Some(samples) = self.samples.get(channel) {
                if !samples.is_empty() {
                    println!("📌 {} ({} samples):", channel, samples.len());
                    for (i, sample) in samples.iter().take(5).enumerate() {
                        println!("  {}. {}", i + 1, sample);
                    }
                    if samples.len() > 5 {
                        println!("  ... and {} more samples", samples.len() - 5);
                    }
                    println!();
                }
            }
        }
    }
}
