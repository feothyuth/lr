//! Test BBO channel reliability vs Order Book channel
//! Compares packet loss rates between the two channels

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::ExampleContext;
use futures_util::StreamExt;
use lighter_client::ws_client::WsEvent;

#[tokio::main]
async fn main() -> Result<()> {
    let ctx = ExampleContext::initialise(Some("test_bbo_reliability")).await?;
    let market = ctx.market_id();

    println!("\n════════════════════════════════════════════════════════════════");
    println!("📊 BBO vs Order Book Reliability Test");
    println!("════════════════════════════════════════════════════════════════\n");

    let mut stream = ctx
        .ws_builder()
        .subscribe_order_book(market)
        .subscribe_bbo(market)
        .connect()
        .await?;

    let mut ob_count = 0u32;
    let mut bbo_count = 0u32;
    let mut ob_last_nonce: Option<u64> = None;
    let mut ob_gaps = 0u32;
    let mut ob_missed = 0u64;

    println!("🔗 Connected! Collecting data for 30 seconds...\n");
    println!("Press Ctrl+C to stop and see results.\n");

    while let Some(event) = stream.next().await {
        match event? {
            WsEvent::OrderBook(update) => {
                ob_count += 1;

                // Track order book nonce gaps
                if let Some(nonce) = update.nonce {
                    if let Some(prev) = ob_last_nonce {
                        let expected = prev + 1;
                        if nonce != expected && nonce > prev {
                            ob_gaps += 1;
                            ob_missed += nonce - expected;
                        }
                    }
                    ob_last_nonce = Some(nonce);
                }

                if ob_count % 100 == 0 {
                    print_status(ob_count, bbo_count, ob_gaps, ob_missed);
                }
            }
            WsEvent::BBO(_) => {
                bbo_count += 1;
            }
            _ => {}
        }
    }

    println!("\n✅ Test Complete!\n");
    print_final_report(ob_count, bbo_count, ob_gaps, ob_missed);

    Ok(())
}

fn print_status(ob_count: u32, bbo_count: u32, ob_gaps: u32, ob_missed: u64) {
    let ob_reliability = if ob_missed > 0 {
        (ob_count as f64 / (ob_count as u64 + ob_missed) as f64) * 100.0
    } else {
        100.0
    };

    println!(
        "📊 OrderBook: {} updates | {} gaps | {:.1}% reliability | BBO: {} events",
        ob_count, ob_gaps, ob_reliability, bbo_count
    );
}

fn print_final_report(ob_count: u32, bbo_count: u32, ob_gaps: u32, ob_missed: u64) {
    let ob_total = ob_count as u64 + ob_missed;
    let ob_reliability = if ob_total > 0 {
        (ob_count as f64 / ob_total as f64) * 100.0
    } else {
        100.0
    };

    println!("════════════════════════════════════════════════════════════════");
    println!("📈 FINAL RESULTS");
    println!("════════════════════════════════════════════════════════════════");
    println!("\n📘 ORDER BOOK CHANNEL:");
    println!("   Received: {} updates", ob_count);
    println!("   Gaps: {}", ob_gaps);
    println!("   Missed: {} updates", ob_missed);
    println!("   Reliability: {:.2}%", ob_reliability);

    println!("\n📗 BBO CHANNEL:");
    println!("   Received: {} events", bbo_count);
    println!("   (BBO has no nonce, can't measure packet loss)");

    println!("\n💡 ANALYSIS:");
    if ob_reliability < 50.0 {
        println!("   ⚠️  POOR connection quality ({:.1}% packet loss)", 100.0 - ob_reliability);
        println!("   Recommendation: Use BBO channel if you only need TOB");
        println!("   Or: Request REST snapshots frequently to resync");
    } else if ob_reliability < 90.0 {
        println!("   ⚠️  MODERATE packet loss ({:.1}%)", 100.0 - ob_reliability);
        println!("   Recommendation: Use nonce-based snapshot refresh");
    } else {
        println!("   ✅ GOOD connection quality ({:.1}% reliability)", ob_reliability);
    }

    println!("\n   BBO message rate: {:.1}% of order book updates",
        (bbo_count as f64 / ob_count as f64) * 100.0);
    println!("════════════════════════════════════════════════════════════════\n");
}
