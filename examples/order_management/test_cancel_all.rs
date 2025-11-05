//! Cancel All Orders via WebSocket
//!
//! Uses the CancelAllOrders API transaction type (type_id = 16)
//! CAUTION: This will cancel every single order you have!

#[path = "../common/example_context.rs"]
mod common;

use anyhow::Result;
use common::{connect_private_stream, sign_cancel_all_for_ws, ExampleContext};
use serde_json::Value;
use std::io::{self, Write as IoWrite};
use std::time::Instant;
use tokio::time::{timeout, Duration};

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n{}", "═".repeat(80));
    println!("🗑️  Test Cancel ALL Orders");
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
    let ctx = ExampleContext::initialise(Some("test_cancel_all")).await?;
    let client = ctx.client();

    println!("✅ Signer initialized\n");

    // Sign cancel all transaction
    let total_start = Instant::now();
    // time_in_force = 0 (ImmediateCancelAll), time = 0 (for immediate)
    println!("✍️  Signing cancel all transaction...");
    let sign_start = Instant::now();

    let (tx_type, tx_info) = sign_cancel_all_for_ws(&ctx, 0, 0).await?;

    let sign_duration = sign_start.elapsed();
    println!(
        "✅ Cancel all transaction signed ({}ms)\n",
        sign_duration.as_millis()
    );

    // Connect to WebSocket
    println!("🌐 Connecting to WebSocket...");
    let builder = client.ws().subscribe_transactions();
    let connect_start = Instant::now();
    let (mut stream, _) = connect_private_stream(&ctx, builder).await?;
    let connect_duration = connect_start.elapsed();
    println!("✅ Connected and authenticated\n");

    // Send transaction (type_id = 16 for CancelAllOrders)
    println!("📤 Sending cancel all transaction...");
    let send_start = Instant::now();

    let payload: Value = serde_json::from_str(&tx_info)?;
    let connection = stream.connection_mut();
    connection.send_transaction(tx_type, payload).await?;

    let send_duration = send_start.elapsed();
    println!("✅ Transaction sent ({}ms)\n", send_duration.as_millis());

    // Wait for response
    println!("⏳ Waiting for response...\n");
    let response_start = Instant::now();

    let outcome = timeout(
        Duration::from_secs(5),
        connection.wait_for_tx_response(Duration::from_secs(5)),
    )
    .await;

    let (success, response_duration) = match outcome {
        Ok(Ok(true)) => {
            let response_duration = response_start.elapsed();
            println!(
                "  ✅ Cancel all successful ({}ms)",
                response_duration.as_millis()
            );
            (true, response_duration)
        }
        Ok(Ok(false)) => {
            let response_duration = response_start.elapsed();
            println!(
                "  ❌ Cancel all rejected ({}ms)",
                response_duration.as_millis()
            );
            (false, response_duration)
        }
        Ok(Err(e)) => {
            let response_duration = response_start.elapsed();
            println!("  ⏳ Error: {} ({}ms)", e, response_duration.as_millis());
            (false, response_duration)
        }
        Err(_) => {
            let response_duration = response_start.elapsed();
            println!("  ⏳ Timeout ({}ms)", response_duration.as_millis());
            (false, response_duration)
        }
    };

    let total_duration = total_start.elapsed();

    println!();
    println!("📊 Latency Summary:");
    println!("   Signing:       {}ms", sign_duration.as_millis());
    println!("   Connect:       {}ms", connect_duration.as_millis());
    println!("   Send:          {}ms", send_duration.as_millis());
    println!("   Response:      {}ms", response_duration.as_millis());
    println!("   ─────────────────────");
    println!("   Total RTT:     {}ms", total_duration.as_millis());
    println!();

    if success {
        println!("✅ ALL orders cancelled successfully!");
    } else {
        println!("⚠️  Cancel all failed - run monitor_active_orders to verify");
    }

    println!("\n{}", "═".repeat(80));
    println!("✅ Test complete!");
    println!("{}\n", "═".repeat(80));

    Ok(())
}
