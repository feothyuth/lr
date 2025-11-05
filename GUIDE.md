# Lighter Client Rust SDK – Developer Guide

This guide explains how to work with the `lighter_client` crate that ships in this repository. It is grounded in the current codebase—no placeholders, no boilerplate. Follow it to configure credentials, use the high-level client, and run the supplied bots and benchmarks.

---

## 1. Repository layout

```
lighter-client-main-rust/
├── src/                     # Library code (REST handles, signer glue, ws helpers)
├── examples/                # Executable samples (REST, WS, bots, benchmarks)
├── signers/                 # Bundled signer shared libs (mac arm64, linux x86_64)
├── config.toml              # Example configuration consumed by examples
├── scripts/                 # Build / lint utilities
└── README.md                # High-level overview
```

The crate exposes:

- `lighter_client::lighter_client::LighterClient` – high-level async client
- `lighter_client::signer_client::SignerClient` – lower-level signer wrapper
- `lighter_client::ws_client` – WebSocket builders/events
- `lighter_client::tx_executor` – WebSocket submission helpers (`send_batch_tx_ws`)
- `lighter_client::types` – strongly typed identifiers (markets, prices, nonces…)

Generated REST modules live under `src/apis/` if you need raw endpoint access.

---

## 2. Requirements

- Rust **1.74+** (stable)
- Tokio (all high-level APIs are async)
- `openssl` headers only if you enable the `native-tls` feature

Install/upgrade:

```bash
rustup default stable
rustup update
rustc --version
```

---

## 3. Installing the crate

Add from another crate:

```toml
[dependencies]
lighter_client = { path = "../lighter-client-main-rust" }
```

### Feature flags

```
default = ["rustls-tls"]
rustls-tls  # TLS via rustls (default)
native-tls  # system TLS (needs OpenSSL on Linux)
timings     # emit signer/submit timing spans
```

---

## 4. Credentials & environment

Examples consume these environment variables:

| Variable | Required | Purpose |
|----------|----------|---------|
| `LIGHTER_PRIVATE_KEY` | ✔ | Hex private key for signing |
| `LIGHTER_ACCOUNT_INDEX` | ✔ | Account id (i64) |
| `LIGHTER_API_KEY_INDEX` | ✔ | API key id (i32) |
| `LIGHTER_API_URL` | ✖ | Defaults to `https://mainnet.zklighter.elliot.ai` |
| `LIGHTER_WS_URL` | ✖ | Override WebSocket host/path |
| `LIGHTER_CONFIG_PATH` | ✖ | Alternate config for examples |
| `LIGHTER_SIGNER_PATH` | ✖ | Point to custom signer `.so/.dylib` |

The helper `examples/common/example_context.rs` falls back to `config.toml` for non-secret defaults such as market id or order size. Secrets **must** remain in env vars.

Example `.env`:

```
LIGHTER_PRIVATE_KEY=0x...
LIGHTER_ACCOUNT_INDEX=281474976665656
LIGHTER_API_KEY_INDEX=0
LIGHTER_API_URL=https://mainnet.zklighter.elliot.ai
```

Load it with `dotenvy` (most examples call `dotenvy::dotenv().ok();`).

---

## 5. Quick usage snippets

### 5.1 Build a client + signer

```rust
use lighter_client::{
    lighter_client::LighterClient,
    types::{AccountId, ApiKeyIndex},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = LighterClient::builder()
        .api_url(std::env::var("LIGHTER_API_URL").unwrap_or_else(|_| "https://mainnet.zklighter.elliot.ai".into()))
        .private_key(std::env::var("LIGHTER_PRIVATE_KEY")?)
        .account_index(AccountId::new(std::env::var("LIGHTER_ACCOUNT_INDEX")?.parse()?))
        .api_key_index(ApiKeyIndex::new(std::env::var("LIGHTER_API_KEY_INDEX")?.parse()?))
        .build()
        .await?;

    if let Some(err) = client.signer().unwrap().check_client().await? {
        anyhow::bail!("signer check failed: {err}");
    }

    println!("client ready");
    Ok(())
}
```

### 5.2 Submit a post-only limit order over WebSocket

```rust
use lighter_client::{
    lighter_client::LighterClient,
    tx_executor::{send_batch_tx_ws, TX_TYPE_CREATE_ORDER},
    types::{MarketId, BaseQty, Price, Nonce},
};

async fn submit(client: &LighterClient) -> anyhow::Result<()> {
    let signer = client.signer().context("client missing signer")?;
    let (api_key, nonce) = signer.next_nonce().await?;

    let market = MarketId::new(1);
    let qty = BaseQty::try_from(2000)?; // depends on market size decimals
    let signed = client
        .order(market)
        .buy()
        .qty(qty)
        .limit(Price::ticks(1_100_000))
        .post_only()
        .with_api_key(api_key.into())
        .with_nonce(Nonce::new(nonce))
        .sign()
        .await?;

    let mut stream = client
        .ws()
        .subscribe_transactions()
        .connect()
        .await?;
    stream.connection_mut().set_auth_token(client.create_auth_token(None)?);

    let entry = signed.into_parts().0;
    let result = send_batch_tx_ws(stream.connection_mut(), vec![(TX_TYPE_CREATE_ORDER, entry.tx_info)]).await?;
    anyhow::ensure!(result.first() == Some(&true), "order rejected");
    Ok(())
}
```

### 5.3 Subscribe to market data

```rust
use lighter_client::{lighter_client::LighterClient, ws_client::WsEvent};

async fn stream_book(client: &LighterClient, market: MarketId) -> anyhow::Result<()> {
    let mut ws = client
        .ws()
        .subscribe_order_book(market)
        .subscribe_trades(market)
        .connect()
        .await?;

    while let Some(evt) = ws.next().await {
        match evt? {
            WsEvent::OrderBook(book) => println!("best bid: {}", book.state.bids[0].price),
            WsEvent::Trade(trade) => println!("trade: {} @ {}", trade.amount, trade.price),
            _ => {}
        }
    }
    Ok(())
}
```

---

## 6. Examples & bots

Examples are grouped by theme:

| Directory | Highlights |
|-----------|------------|
| `examples/quickstart/` | REST + WS basics (`01_quickstart`, `07_websocket_transaction`, …) |
| `examples/monitoring/` | Market monitors (`monitor_active_orders`, `multi_channel_monitor`) |
| `examples/order_management/` | Order flows (`test_simple_limit`, `test_cancel_all`, …) |
| `examples/trading/` | Bot scaffolding (`simple_order_refresher`, `simple_twoside_mm`) |
| `examples/benchmarks/` | Latency studies (`benchmark_batch_submit_v2`, `bench_ws_vs_rest`) |

Run an example (release mode recommended for bots/benchmarks):

```bash
LIGHTER_PRIVATE_KEY=... \
LIGHTER_ACCOUNT_INDEX=... \
LIGHTER_API_KEY_INDEX=... \
cargo run --release --example simple_twoside_mm
```

The market-maker (`simple_twoside_mm.rs`) demonstrates:

- fee-aware spread floor (maker fee + configurable edge)
- mark clamped to raw mid (±25% live spread)
- pure tick maths (floor/ceil) with adaptive post-only slack
- nonce resynchronisation on batch rejection
- fast timer + mark-triggered requotes so quotes stay glued to the book

Benchmarks in `examples/benchmarks/` compare batch vs single submission latency and WS vs REST transactions.

---

## 7. Signer notes

- Bundled signer `.so/.dylib` files live in `signers/` (loaded automatically for linux x86_64 + mac arm64).
- Override via `LIGHTER_SIGNER_PATH` or `.signer_library_path()` when shipping a custom signer build.
- Nonce management defaults to the optimistic manager. Switch to strict REST-backed retrieval with `NonceManagerType::Api` if you run multiple writers on the same API key.
- `signer_client` exposes helpers for every transaction (`sign_create_order`, `sign_cancel_order`, `sign_withdraw`, `sign_cancel_all_orders`, …). Each returns a `SignedPayload<T>` or a raw payload string.

If a batch returns `invalid nonce`, call `client.account().next_nonce(api_key)` and rebuild the batch—see the bot examples for a ready-made pattern.

---

## 8. Working with WebSockets

- Compose subscriptions with `lighter_client::ws_client::WsBuilder`.
- `WsStream` yields `WsEvent` variants (`OrderBook`, `Trade`, `MarketStats`, `Account`, …).
- Set your auth token (`client.create_auth_token(None)`) on `WsConnection` before listening to private channels.
- Use `send_batch_tx_ws` for WebSocket submissions; it returns `Vec<bool>` indicating per-transaction ack success.

---

## 9. Testing & quality gates

Before committing:

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features --workspace
```

Benchmarks and bots are opt-in:

```bash
cargo run --release --example benchmark_batch_submit_v2
cargo run --release --example simple_twoside_mm
```

---

## 10. Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `signer error: unsupported platform` | Unsupported architecture (e.g. aarch64 server) | Build signer for target arch and set `LIGHTER_SIGNER_PATH` |
| `invalid nonce` after batch submit | Optimistic nonce drift | Call `client.account().next_nonce()`, then resend (bots already do this) |
| Post-only rejections | Rounding bids up / asks down or clamping vs peer top | Use floor/ceil helpers, clamp vs raw BBO, add 1–2 tick slack |
| Spreads widen every refresh | Compounding live spread | Use fee-aware fixed `spread_pct` (see `simple_twoside_mm`) |
| WebSocket disconnects | JSON pong missing | Use `WsConnection` from this crate (ping/pong handled) |

Logging tip:

```bash
RUST_LOG=lighter_client=debug,reqwest=info cargo run --example 01_quickstart
```

Enable the `timings` feature for built-in latency spans.

---

## 11. Contributing workflow

1. Create a feature branch.
2. Run fmt + clippy + tests (see §9).
3. Update examples/tests/docs when behavior changes.
4. Submit a PR describing your change and how you validated it.

---

## 12. Additional resources

- `README.md` – project highlights and module map.
- `examples/trading/simple_twoside_mm.rs` – production-style WS maker (center clamp, PO guard, nonce resync).
- `examples/benchmarks/` – latency benchmarking harnesses.
- `src/tx_executor.rs` – WebSocket batch submission helpers.
- `src/lighter_client/client.rs` – typed REST handles and order builders.

Happy building!
