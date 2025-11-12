# Avellaneda Strategy – Quickstart (5 Minutes)

This guide gets you from a clean checkout to a runnable Avellaneda-Stoikov strategy sample in five minutes. Follow the numbered sections in order.

---

## 1. Initialize Dependencies

```bash
cd /mnt/c/Users/randa/Documents/bots/MM_Avellaneda/Rust_SDK
cargo add statrs ringbuffer anyhow serde serde_json tokio --features time
```

`statrs` powers the math library, `ringbuffer` stores rolling prices, and the remaining crates align the project with the implementation roadmap.

---

## 2. Scaffold Project Structure

```bash
mkdir -p src/avellaneda
mkdir -p examples/trading/avellaneda
mkdir -p tests docs

touch src/avellaneda/{mod,inventory,volatility,market_data,spreads,execution,types,config}.rs
touch examples/trading/avellaneda/main.rs
```

This mirrors the structure laid out in the implementation plan (Section 2).

---

## 3. Configure `Cargo.toml`

Add the example target and feature gate near the end of `Cargo.toml`:

```toml
[features]
default = []
backtest = []

[[example]]
name = "mm_avellaneda"
path = "examples/trading/avellaneda/main.rs"
required-features = ["default"]
```

Confirm the new dependencies appear under `[dependencies]`. If `cargo add` failed (no network), open `Cargo.toml` and manually insert:

```toml
statrs = "0.16"
ringbuffer = "0.12"
anyhow = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
```

---

## 4. Create `config.toml`

Use this starter template (copy/paste):

```toml
[avellaneda]
market_id = 1
dry_run = true
order_size = 0.001
gamma = 0.1
kappa = 1.5
time_horizon_hours = 1.0
target_base_pct = 0.5
vol_lookback = 100
vol_ewma_alpha = 0.1
refresh_interval_ms = 200
min_spread_bps = 5.0
max_spread_bps = 100.0
max_position = 0.01
min_notional = 10.0
volatility_breaker = 2.5
```

Add environment overrides only after your first dry-run succeeds.

---

## 5. Drop in the First Smoke Test

Create `tests/formulas.rs` with the following content:

```rust
use lighter_client::avellaneda::spreads::{optimal_spread, reservation_price};

#[test]
fn reservation_price_is_symmetric_when_flat() {
    let mid = 100_000.0;
    let sigma = 0.5; // 50% annual volatility
    let gamma = 0.1;
    let kappa = 1.5;
    let time_left = 1.0;

    let r = reservation_price(mid, 0.0, gamma, sigma, time_left);
    let delta = optimal_spread(gamma, sigma, time_left, kappa);
    let bid = r - delta / 2.0;
    let ask = r + delta / 2.0;
    assert!((mid - bid - (ask - mid)).abs() < 1e-6);
    assert!(ask > bid);
}
```

Run the test:

```bash
cargo test reservation_price_is_symmetric_when_flat
```

Green tests confirm your formulas are wired correctly before integrating order flow.

---

## 6. Launch the Example (Dry Run)

Populate `examples/trading/avellaneda/main.rs` with a minimal stub that loads config and prints quotes (refer to Section 11 in the plan), then run:

```bash
AVELLANEDA_DRY_RUN=1 cargo run --example mm_avellaneda
```

You should see recurring log lines like:
```
INFO  quote: mid=102400.00 r=102399.99 δ=15.20 | bid=102392.39 ask=102407.59 | q=0.00 σ=0.48
```

---

## 7. Common Issues & Quick Fixes

- **`cargo add` fails (no network)**: manually edit `Cargo.toml` as shown in Section 3, then run `cargo fetch` later when access is restored.
- **Tests cannot find modules**: ensure `src/avellaneda/mod.rs` re-exports the submodules, e.g., `pub mod spreads;`.
- **Dry run panics on config**: verify `config.toml` keys match the names expected in `AvellanedaConfig` (Section 9 of the plan).
- **Quotes not updating**: check that `refresh_interval_ms` > 0 and the Tokio runtime in the example uses `#[tokio::main(flavor = "multi_thread")]`.
- **Spreads collapse to 0**: seed `volatility_sigma` with a floor (10 bps) until enough samples arrive; see plan Section 4.

Once these steps are complete, continue with Phase 1 of the implementation roadmap.
