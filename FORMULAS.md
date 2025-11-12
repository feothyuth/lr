# Avellaneda-Stoikov Formula Reference

This reference card distills every mathematical primitive used by `mm_avellaneda.rs`. For each formula you will find an explanation, calculation steps, a Rust template, parameter guidance, and a numeric example. The six formulas match Section 11 of the implementation roadmap.

---

## Parameter Cheat Sheet

| Parameter | Symbol | Typical Range | Default | Effect |
|-----------|--------|---------------|---------|--------|
| Risk aversion | γ | 0.05 – 0.5 | 0.1 | Wider spreads and stronger inventory mean reversion as γ ↑ |
| Liquidity | κ | 0.5 – 3.0 | 1.5 | Wider spreads in thin books (κ ↓) |
| Time horizon (hours) | T | 0.25 – 4.0 | 1.0 | Longer horizons keep spreads wider longer |
| Target base pct | τ\_target | 0.3 – 0.7 | 0.5 | Skews inventory preference between base/quote |
| Volatility lookback | N | 50 – 200 | 100 | Controls EWMA responsiveness |
| Volatility EWMA α | α | 0.05 – 0.3 | 0.1 | Higher α reacts faster to new volatility |

Safety bounds: `min_spread_bps = 5`, `max_spread_bps = 100`, `max_position = 0.01`, `volatility_breaker = 2.5`.

---

## Formula 1 – Normalized Inventory (q)

**Purpose:** express inventory as a unitless fraction of the total portfolio, enabling symmetric risk treatment across instruments.

**Formula**
```
q = (B_actual - B_target) / B_total

B_actual  = current base balance
B_target  = target base balance = target_pct × total_value / mid
B_total   = total portfolio in base units = total_value / mid
total_value = base_balance × mid + quote_balance
```

**Calculation Steps**
1. Compute total portfolio value in quote currency.
2. Derive target base units using `target_base_pct`.
3. Convert portfolio back to base units.
4. Divide the inventory delta by total base units.

**Rust Template**
```rust
pub fn normalized_inventory(
    base_balance: f64,
    quote_balance: f64,
    mid: f64,
    target_base_pct: f64,
) -> f64 {
    let total_value = base_balance * mid + quote_balance;
    if total_value <= f64::EPSILON {
        return 0.0;
    }
    let target_base = (total_value * target_base_pct) / mid;
    let total_base = total_value / mid;
    (base_balance - target_base) / total_base
}
```

**Example**
- Base balance = 1.0 BTC
- Quote balance = 50,000 USDC
- Mid = 100,000 USDC/BTC
- Target pct = 0.5

Total value = 1.0 × 100,000 + 50,000 = 150,000  
Target base = (150,000 × 0.5) / 100,000 = 0.75 BTC  
Total base = 150,000 / 100,000 = 1.5 BTC  
`q = (1.0 - 0.75) / 1.5 = 0.1667` (16.7% long)

---

## Formula 2 – Time Horizon Remaining (τ)

**Purpose:** scale risk terms by the remaining fraction of the planning horizon.

**Formula**
```
τ = max(T_total - t_elapsed, ε)

T_total   = time_horizon_hours × 3600
t_elapsed = now - session_start
ε         = small positive floor (0.01 seconds)
```

**Rust Template**
```rust
pub fn time_left_seconds(start: Instant, horizon_hours: f64) -> f64 {
    let total = horizon_hours * 3600.0;
    let elapsed = start.elapsed().as_secs_f64();
    (total - elapsed).max(0.01)
}
```

**Example**
- Horizon = 1 hour (3600s)
- Elapsed = 900s (15 minutes)

`τ = max(3600 - 900, 0.01) = 2700 seconds`

Use `τ` directly in subsequent formulas; divide by 3600 to obtain hours if required.

---

## Formula 3 – EWMA Volatility (σ)

**Purpose:** estimate current volatility from rolling mid-price samples.

**Formula**
```
r_i     = ln(P_i / P_{i-1})              (log-return)
σ_sample = sqrt(Σ(r_i - μ)² / n)         (standard deviation)
σ_t     = α × σ_sample + (1 - α) × σ_{t-1}
σ_ann   = σ_t × √K                       (annualized; optional)
```

`K` equals the number of samples per year (e.g., quotes_per_seconds × 86,400 × 365).

**Rust Template**
```rust
pub struct VolEstimator {
    prices: AllocRingBuffer<f64>,
    alpha: f64,
    sigma: f64,
}

impl VolEstimator {
    pub fn on_mid_price(&mut self, mid: f64) {
        if let Some(prev) = self.prices.back() {
            let log_return = (mid / *prev).ln();
            self.prices.push(mid);
            let mean = self.prices
                .iter()
                .zip(self.prices.iter().skip(1))
                .map(|(a, b)| (b / a).ln())
                .sum::<f64>()
                / (self.prices.len().saturating_sub(1) as f64);
            let variance = self.prices
                .iter()
                .zip(self.prices.iter().skip(1))
                .map(|(a, b)| {
                    let r = (b / a).ln();
                    (r - mean).powi(2)
                })
                .sum::<f64>()
                / (self.prices.len().saturating_sub(1) as f64);
            let sample_sigma = variance.max(0.0).sqrt();
            self.sigma = self.alpha * sample_sigma + (1.0 - self.alpha) * self.sigma.max(1e-4);
        } else {
            self.prices.push(mid);
        }
    }

    pub fn sigma(&self) -> f64 {
        self.sigma
    }
}
```

**Example**
- Mid prices (USDC): 100000, 100020, 99980, 100050
- Log returns ≈ `[0.0002, -0.0004, 0.0007]`
- `σ_sample ≈ 0.00045`
- With α = 0.1 and previous σ = 0.0005: `σ_new = 0.1 × 0.00045 + 0.9 × 0.0005 = 0.000495`

Annualize for reporting: if 5 samples per second → `K = 5 × 86400 × 365 ≈ 157,680,000`, so `σ_ann ≈ 0.000495 × √K ≈ 0.495` (49.5% annual volatility).

---

## Formula 4 – Reservation Price (r)

**Purpose:** shift fair value away from the mid price based on inventory imbalance.

**Formula**
```
r = S - q × γ × σ × τ

S  = current mid price
q  = normalized inventory
γ  = risk aversion
σ  = volatility estimate (per second or per hour)
τ  = time remaining (seconds)
```

**Rust Template**
```rust
pub fn reservation_price(
    mid: f64,
    q: f64,
    gamma: f64,
    sigma: f64,
    time_left: f64,
) -> f64 {
    mid - q * gamma * sigma * time_left
}
```

**Example**
- `S = 100,000`
- `q = 0.1667`
- `γ = 0.1`
- `σ = 0.000495` (per second)
- `τ = 2700 seconds`

`r = 100,000 - 0.1667 × 0.1 × 0.000495 × 2700 ≈ 99,997.77`

The reservation price is below mid because inventory is long.

---

## Formula 5 – Optimal Spread (δ)

**Purpose:** determine half-spread width that balances risk versus execution probability.

**Formula**
```
δ = γ × σ × τ + (2/γ) × ln(1 + γ/κ)
```

**Interpretation**
- First term widens with volatility, risk aversion, and time remaining.
- Second term widens when market liquidity (κ) deteriorates.

**Rust Template**
```rust
pub fn optimal_spread(
    gamma: f64,
    sigma: f64,
    time_left: f64,
    kappa: f64,
) -> f64 {
    let risk_term = gamma * sigma * time_left;
    let liquidity_term = (2.0 / gamma) * (1.0 + gamma / kappa).ln();
    risk_term + liquidity_term
}
```

**Example**
- `γ = 0.1`
- `σ = 0.000495`
- `τ = 2700`
- `κ = 1.5`

Risk term = `0.1 × 0.000495 × 2700 = 0.13365`  
Liquidity term = `(2/0.1) × ln(1 + 0.1/1.5) = 20 × ln(1.0667) = 20 × 0.0645 = 1.290`  
`δ = 0.13365 + 1.290 = 1.42365` (quote currency units)

---

## Formula 6 – Quote Prices (bid/ask)

**Purpose:** transform reservation price and spread into actionable bid/ask quotes.

**Formula**
```
bid = r - δ/2
ask = r + δ/2
```

**Rust Template**
```rust
pub fn generate_quotes(
    reservation_price: f64,
    optimal_spread: f64,
) -> (f64, f64) {
    let half = optimal_spread / 2.0;
    (reservation_price - half, reservation_price + half)
}
```

**Example**
- `r = 99,997.77`
- `δ = 1.42365`

`bid = 99,997.77 - 0.71183 = 99,997.06`  
`ask = 99,997.77 + 0.71183 = 99,998.48`  
Spread = `1.42365` (≈ 14.2 bps at 100k mid).

Apply tick-size rounding and clamp to safety bounds before submission.

---

## Worked Example (BTC-USDC)

Given:
- Mid price `S = 100,000`
- Base balance `1.0 BTC`, quote balance `50,000 USDC`
- `γ = 0.1`, `κ = 1.5`, `time_horizon_hours = 1.0`
- `target_base_pct = 0.5`
- `vol_lookback = 100`, `α = 0.1`
- Latest volatility output `σ = 0.000495` (per second)
- Time since start `900s`

1. **Normalized inventory** → `q = 0.1667`
2. **Time left** → `τ = 2700 seconds`
3. **Reservation price** → `r = 99,997.77`
4. **Optimal spread** → `δ = 1.42365`
5. **Quotes** → `bid = 99,997.06`, `ask = 99,998.48`

Rounded to nearest 0.1 USDC tick:
- `bid = 99,997.1`
- `ask = 99,998.4`

Final spread = `1.3` USDC (13 bps). Because the inventory is long, both quotes shift slightly downward, incentivizing sells.

---

## Implementation Checklist

- Seed volatility estimator with a non-zero floor (`sigma >= 1e-4`) to avoid divide-by-zero.
- Clamp `δ` within `[min_spread_bps, max_spread_bps]` after conversion to bps.
- Ensure `bid < ask` after rounding; otherwise widen by one tick on each side.
- Feed the same `τ` into both reservation price and optimal spread to preserve theoretical consistency.

These formulas underpin every phase of the roadmap. Confirm each unit test in `tests/formulas.rs` mirrors the examples above before moving on to integration work.
