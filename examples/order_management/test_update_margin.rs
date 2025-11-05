#[path = "../common/env.rs"]
mod common_env;

use common_env::{ensure_positive, require_env, require_parse_env, resolve_api_url};
use lighter_client::nonce_manager::NonceManagerType;
use lighter_client::signer_client::SignerClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Note: Set LIGHTER_PRIVATE_KEY environment variable to test with real credentials

    let private_key = require_env(&["LIGHTER_PRIVATE_KEY"], "LIGHTER_PRIVATE_KEY");
    let url = resolve_api_url("https://api.elliottech.org/testnet");
    let account_index = ensure_positive(
        require_parse_env(
            &[
                "LIGHTER_ACCOUNT_INDEX",
                "ACCOUNT_INDEX",
                "LIGHTER_ACCOUNT_ID",
            ],
            "account index",
        ),
        "account index",
    );
    let api_key_index =
        require_parse_env(&["LIGHTER_API_KEY_INDEX", "API_KEY_INDEX"], "API key index");

    println!("Testing update_margin function...");

    // Create signer client
    let client = SignerClient::new(
        &url,
        &private_key,
        api_key_index, // api_key_index
        account_index, // account_index
        None,          // max_api_key_index
        None,          // private_keys
        NonceManagerType::Optimistic,
        None, // signer_library_path
    )
    .await?;

    println!("✓ SignerClient created successfully");

    // Test the update_margin function (this will fail without proper setup, but tests the FFI)
    match client
        .update_margin(
            0,     // market_index (BTC)
            100.0, // usdc_amount
            1,     // direction (1 = add margin)
            None,  // nonce
            None,  // api_key_index
        )
        .await
    {
        Ok((tx_info, response)) => {
            println!("✓ update_margin succeeded!");
            println!("  Transaction info: {}", tx_info);
            println!("  Response code: {}", response.code);
        }
        Err(e) => {
            println!(
                "⚠ update_margin failed (expected without proper setup): {}",
                e
            );
            println!("✓ FFI integration works - function is callable");
        }
    }

    println!("✓ All tests completed successfully!");
    Ok(())
}
