# Strategy Comparison: `mm_hawkes` vs `mm_avellaneda`

Choosing between the legacy Hawkes-process market maker and the new Avellaneda-Stoikov implementation hinges on market structure, operational priorities, and team bandwidth. Use this decision guide to select and migrate confidently.

---

## 1. When to Use Each Strategy

**Pick `mm_avellaneda` when**
- Market is relatively efficient, deep, and mean-reverting (BTC/ETH majors, blue-chip perps).
- You value mathematical transparency and cleaner code (~800 LOC).
- Quick parameter tuning (5 knobs) is preferred over exhaustive heuristics.
- Latency target is ≥200 ms and adverse selection is manageable.

**Pick `mm_hawkes` when**
- Order flow is toxic (aggressive takers, oscillations, crash risk).
- You require built-in defenses: FlipGate, mark-out checks, crash detectors.
- Latency budget is <50 ms with 20 ms refresh cadence.
- You already depend on bespoke heuristics tuned for adversarial venues.

---

## 2. Side-by-Side Feature Comparison

| Capability | `mm_hawkes` | `mm_avellaneda` |
|------------|-------------|-----------------|
| Code size | 1,500+ LOC | ~800 LOC |
| Core model | Hawkes intensity + heuristics | Closed-form stochastic control |
| Parameters | 20+ intertwined | 5 core + 3 safety bounds |
| Inventory handling | Same-side throttles, cooldowns | Normalized inventory skew |
| Volatility | 2 s bar EWMA + toxicity flags | Rolling EWMA (`vol_lookback`) |
| Defenses | FlipGate, crash detector, mark-out | Spread clamps, vol breaker, kill switch |
| Refresh cadence | 20 ms (50 Hz) | 200 ms (5 Hz) default |
| Best markets | Volatile, toxic, thin | Liquid, orderly, high-volume |
| Maintenance load | High (many edge cases) | Low (formula-driven) |

---

## 3. Performance Expectations

| Profile | `mm_hawkes` | `mm_avellaneda` |
|---------|-------------|-----------------|
| Conservative | 0.8–1.5% daily, high safety | 0.5–1.0% daily, low drawdown |
| Moderate | 1.5–3.0% daily (needs tuning) | 1.0–2.0% daily with γ=0.1 |
| Aggressive | 3.0%+ daily but riskier | 2.0–3.5% daily with γ=0.05 |
| Latency target | <20 ms | 100–250 ms |
| Failure modes | Config drift, over-fitting heuristics | Adverse selection in toxic markets |

Assume the same market data feed and inventory limits. Avellaneda’s returns rely on accurate volatility estimates; Hawkes relies on precise toxicity thresholds.

---

## 4. Parameter Complexity Analysis

**`mm_hawkes` buckets (~24 params)**
- Timing: refresh intervals, watchdog timers, resub delays.
- Defensive: FlipGate toggles, crash thresholds, same-side refill cooldowns.
- Trading: base spread, skew multipliers, inventory brakepoints.

**`mm_avellaneda` knobs (5 core + 3 safety)**
- Core: `gamma`, `kappa`, `time_horizon_hours`, `target_base_pct`, `vol_lookback`, `vol_ewma_alpha`.
- Safety: `min_spread_bps`, `max_spread_bps`, `max_position`, `volatility_breaker`.

**Takeaway:** Hawkes parameters are multi-dimensional and interdependent; Avellaneda’s are orthogonal and approachable. Expect 3–4 hours to re-tune Hawkes versus 30 minutes for Avellaneda.

---

## 5. Migration Paths

**Hawkes → Avellaneda (simplify)**
1. Run Avellaneda in dry-run alongside Hawkes for one trading day.
2. Match position caps and order sizes; compare spread/PNL metrics.
3. Cut over on liquid pairs; keep Hawkes on standby for toxic symbols.
4. Retire unused Hawkes cron jobs (FlipGate alerts, crash GUI) to reduce noise.

**Avellaneda → Hawkes (add defenses)**
1. Document which issues triggered rollback (e.g., getting picked off).
2. Enable Hawkes with conservative defaults; keep Avellaneda live in sandbox for regression tests.
3. Incrementally enable defenses (FlipGate → crash detector → mark-out) while monitoring fill ratios.
4. Clone Avellaneda’s configuration into Hawkes for inventory targets and notional limits.

**Hybrid approach:** operate Avellaneda as default and auto-switch to Hawkes when volatility > 3× rolling median or when mark-out loss exceeds threshold.

---

## 6. Decision Tree Flowchart

```
Start
  │
  ├─ Is the market efficient (tight spreads, deep books)?
  │     ├─ Yes → Use mm_avellaneda
  │     └─ No  → ↓
  │
  ├─ Are fills toxic (negative mark-out, oscillations)?
  │     ├─ Yes → Use mm_hawkes
  │     └─ No  → ↓
  │
  ├─ Do you require <50 ms end-to-end latency?
  │     ├─ Yes → Use mm_hawkes
  │     └─ No  → ↓
  │
  ├─ Is rapid iteration / small team bandwidth a priority?
  │     ├─ Yes → Use mm_avellaneda
  │     └─ No  → ↓
  │
  └─ Are regulatory / ops constraints happier with simpler code?
        ├─ Yes → Use mm_avellaneda
        └─ No  → Use mm_hawkes with full defenses
```

---

## 7. Quick Reference Actions

- **Starting from scratch?** Begin with `mm_avellaneda`, rely on the implementation roadmap, and only escalate to Hawkes if metrics show persistent toxic flow.
- **Running both?** Route majors (BTC, ETH) through Avellaneda; keep Hawkes on volatile alts or news-driven events.
- **Need production confidence?** Deploy Avellaneda canary first; maintain Hawkes as a kill-switch fallback.

Use the simplest strategy that satisfies risk appetite; complexity in Hawkes is optional overhead unless the venue forces it.
