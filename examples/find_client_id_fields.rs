//! Find which WebSocket channels have ask_client_id and bid_client_id fields
//! Tests all trade-related channels to find the new fields

#[path = "common/example_context.rs"]
mod common;

use anyhow::Result;
use common::ExampleContext;
use futures_util::StreamExt;
use lighter_client::ws_client::WsEvent;
use serde_json::Value;

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("find_client_id_fields")).await?;
    let market = ctx.market_id();
    let account = ctx.account_id();

    println!("\n════════════════════════════════════════════════════════════════");
    println!("🔍 Searching for ask_client_id / bid_client_id fields");
    println!("════════════════════════════════════════════════════════════════\n");

    println!("📡 Subscribing to trade channels:");
    println!("   ✓ trade/{} (public trades)", market.into_inner());
    println!("   ✓ account_market_trades/{}:{} (your trades)", market.into_inner(), account.into_inner());
    println!("\nWaiting for trades to occur...\n");
    println!("💡 TIP: Place some orders to generate trades!\n");

    let mut stream = ctx
        .ws_builder()
        .subscribe_trade(market)
        .subscribe_account_market_trades(market, account)
        .connect()
        .await?;

    let mut public_trade_count = 0;
    let mut account_market_trade_count = 0;
    let mut found_client_id = false;

    while let Some(event) = stream.next().await {
        match event? {
            WsEvent::Connected => {
                println!("🔗 Connected to Lighter WebSocket\n");
            }
            WsEvent::Trade(trades) => {
                public_trade_count += trades.trades.len();

                if public_trade_count > 0 && public_trade_count % 10 == 0 {
                    println!("📊 Public trades received: {}", public_trade_count);
                }
            }
            WsEvent::AccountMarketTrades(envelope) => {
                account_market_trade_count += 1;

                println!("\n🎯 account_market_trades event #{}", account_market_trade_count);

                // Get the raw JSON
                let raw_json = envelope.event.into_inner();
                check_for_client_id_fields("account_market_trades channel", &raw_json, &mut found_client_id);
                print_full_json(&raw_json);

                if found_client_id {
                    println!("\n✅ FOUND THE FIELDS! Stopping search.\n");
                    break;
                }
            }
            WsEvent::Pong => {}
            other => {
                // Ignore other events
                let _ = other;
            }
        }

        if found_client_id {
            println!("\n✅ FOUND THE FIELDS! Stopping search.\n");
            break;
        }
    }

    println!("\n════════════════════════════════════════════════════════════════");
    println!("📊 SUMMARY");
    println!("════════════════════════════════════════════════════════════════");
    println!("Public trades received: {}", public_trade_count);
    println!("Account market trades: {}", account_market_trade_count);

    if found_client_id {
        println!("\n✅ Found ask_client_id/bid_client_id fields!");
    } else {
        println!("\n❌ Did NOT find ask_client_id/bid_client_id fields");
        println!("   Possible reasons:");
        println!("   1. Fields not deployed yet by Lighter");
        println!("   2. No trades occurred during test");
        println!("   3. Fields only appear on specific order types");
    }
    println!("════════════════════════════════════════════════════════════════\n");

    Ok(())
}

fn check_for_client_id_fields(channel: &str, json: &Value, found: &mut bool) {
    let has_ask_client_id = json.get("ask_client_id").is_some() ||
                            check_nested_field(json, "ask_client_id");
    let has_bid_client_id = json.get("bid_client_id").is_some() ||
                            check_nested_field(json, "bid_client_id");

    if has_ask_client_id || has_bid_client_id {
        println!("\n🎉 FOUND CLIENT ID FIELDS in {}!", channel);
        if has_ask_client_id {
            println!("   ✅ ask_client_id: {:?}", json.get("ask_client_id"));
        }
        if has_bid_client_id {
            println!("   ✅ bid_client_id: {:?}", json.get("bid_client_id"));
        }
        *found = true;
    }
}

fn check_nested_field(json: &Value, field_name: &str) -> bool {
    // Check in nested objects
    if let Some(obj) = json.as_object() {
        for (_key, value) in obj {
            if let Some(nested) = value.as_object() {
                if nested.contains_key(field_name) {
                    return true;
                }
            }
            if let Some(arr) = value.as_array() {
                for item in arr {
                    if let Some(obj) = item.as_object() {
                        if obj.contains_key(field_name) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

fn print_full_json(json: &Value) {
    println!("📄 Full event data:");
    println!("{}", serde_json::to_string_pretty(json).unwrap_or_else(|_| "parse error".to_string()));
    println!();
}
