//! Cancel All Orders via REST API
//!
//! Uses HTTP POST instead of WebSocket for latency comparison
//! CAUTION: This will cancel every single order you have!

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::ExampleContext;
use std::io::{self, Write as IoWrite};
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("🗑️  Test Cancel ALL Orders (REST API)");
    println!("⚠️  WARNING: This will cancel EVERY order on your account!");
    println!("{}\n", "═".repeat(80));

    // Confirmation prompt
    print!("Are you sure you want to continue? (yes/no): ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    if input.trim().to_lowercase() != "yes" {
        println!("\n❌ Cancelled - no orders were cancelled");
        return Ok(());
    }

    println!();

    // Initialize context
    let ctx = ExampleContext::initialise(Some("test_cancel_all_rest")).await?;
    let signer = ctx.signer()?;

    println!("✅ Signer initialized\n");

    // Sign and send cancel all transaction via REST
    // time_in_force = 0 (ImmediateCancelAll), time = 0 (for immediate)
    println!("✍️  Signing and sending cancel all transaction via REST...");
    let total_start = Instant::now();

    // The cancel_all_orders method signs and sends via REST in one call
    let result = signer
        .cancel_all_orders(0, 0, None, None)
        .await
        .map_err(|e| anyhow::anyhow!("Cancel all failed: {}", e));

    let total_duration = total_start.elapsed();

    match result {
        Ok((_tx_info, response)) => {
            println!(
                "✅ Cancel all successful ({}ms)",
                total_duration.as_millis()
            );
            println!("   Response: {:?}\n", response);

            println!("📊 Latency Summary (REST API):");
            println!("   Total RTT:     {}ms", total_duration.as_millis());
            println!();

            println!("✅ ALL orders cancelled successfully!");
        }
        Err(err) => {
            println!("❌ Cancel all failed ({}ms)", total_duration.as_millis());
            println!("   Error: {}\n", err);

            println!("📊 Latency Summary (REST API):");
            println!("   Total RTT:     {}ms", total_duration.as_millis());
            println!();

            println!("⚠️  Cancel all failed - run monitor_active_orders to verify");
            return Err(err);
        }
    }

    println!("\n{}", "═".repeat(80));
    println!("✅ Test complete!");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
