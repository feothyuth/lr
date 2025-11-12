# Hybrid Avellaneda × Hawkes Implementation Roadmap

This document translates the GPT-Pro plan into concrete engineering tasks we can execute
incrementally. Each phase should leave the tree compiling and the bot runnable (at least
in dry-run) before proceeding.

---

## Phase 0 — Foundations (DONE/IN PROGRESS)

| Task | Status | Notes |
| --- | --- | --- |
| Snapshot current workspace | ✅ | `Rust_SDK_backup_20251106T0918/` |
| Tighten logs (transaction acks) | ✅ | Success logs moved to `debug` |
| Reduce `min_spread_bps` to 1.5 | ✅ | Config update |

---

## Phase A — Observability & Guardrails

1. **Per-fill logging**
   - Implement CSV logger capturing: timestamp, side, liquidity role, price, size,
     inventory before/after, mid, mid+1s, 1s markout (bps), same-side fills (1s),
     flip count (200 ms), inventory age.
   - File location: `logs/fills/{timestamp}.csv`.

2. **Rolling metrics**
   - Build metrics struct with rolling windows:
     - Median 1s markout per side (200-fill window).
     - Same-side fills/sec (95th percentile).
     - Inventory age (median & 95th percentile).
     - Net P&L per $1M notional (rolling 30–60 min).

3. **Kill-switches**
   - Per-side: markout < −0.10 bps → pause side 2–5 s.
   - Global: net P&L per $1M < −0.30 bps → halt strategy.

Deliverable: `metrics/` module plus integration into trade loop & execution engine.

---

## Phase B — Economics Floor

1. Normalize fee units (maker fee fraction).
2. Introduce `min_edge_bps_total` config (default 1.1–1.5 bps).
3. Spread computation: `total_bps = max(as_total_bps, min_edge_bps_total)` then adjust
   bid/ask by `total_bps/2 + fee`.

Goal: Never quote below break-even after fees + toxicity buffer.

---

## Phase C — Microstructure Gates

Implement `SideState` and associated guards:

1. **No same-side immediate refill**
   - Cooldown 300–800 ms depending on toxicity.
   - Cap same-side fills at ≤3/s (95th); pause offending side 0.5–1 s if exceeded.

2. **Inventory ladder**
   - Ratio thresholds: 0.25 / 0.50 / 0.75 / 1.0.
   - Heavy-side offsets: +1 / +3 / +5 / +8 ticks.
   - Heavy-side size multipliers: 1.0 / 0.5 / 0.25 / 0.1×.

3. **Flip-rate gate**
   - Track micro-price flips (200 ms window).
   - If ≥8 flips/200 ms: pause both sides or widen 2–4 ticks until calm.

4. **Age-based partial exits**
   - If inventory age > 8 s and price ≥ 6–8 ticks adverse, cross ⅓–½ of position.

5. **Per-side kill switch**
   - Driven by Phase A metrics; disables quoting on toxic side for 2–5 s.

Deliverable: `participation/` module with state machine, integrated ahead of order
builder.

---

## Phase D — Avellaneda–Stoikov Quoting Layer

1. Volatility EWMA (60–120 s) on mid returns.
2. Liquidity parameter κ (start constant, later fit to fills).
3. Risk aversion γ tuned so 1× order size shift moves reservation price 2–4 ticks.
4. Compute reservation price `r` and AS spread `δ_total`.
5. Merge with economics floor & gate adjustments before emitting `QuoteIntent`.

Deliverable: Replace current spread generator with AS module returning `QuoteIntent`
struct suitable for async pipeline.

---

## Phase E — Asynchronous Execution Pipeline

Split the monolithic loop into tasks communicating via bounded `tokio::mpsc` channels:

1. **MarketData Task** → publishes `MarketTick`.
2. **Decision Task** → consumes ticks, runs AS + gates, emits `Action`s.
3. **Order Builder Task** → converts `Action` to minimal diffs against `PendingOrders`.
4. **Tx Sender Task** → signs (using pre-signed cache when available) & submits batches.
5. **Ack/Reconciler Task** → processes `update/transaction`, updates pending map,
   applies fills, triggers kill-switches.

Additional requirements:
   - Maintain `PendingOrders` map keyed by order id / side.
   - Rate-limit in Tx Sender, not Decision.
   - Support cancel-one / replace instead of blanket cancel-all.

Deliverables:
   - New modules under `src/pipeline/`.
   - Refactored example integrating tasks, graceful shutdown & supervisor restart.

---

## Phase F — Speed Polish (optional / after stabilization)

1. **Pre-signed payload cache**
   - Maintain signed cancel/order templates for ±N ticks.
   - Background refresh when params change.

2. **Parallel order slots**
   - Double-buffer quotes (slot A/B) to amortize ack latency.
   - Only cancel stale slot when new slot is acknowledged.

3. **Optimistic fire-and-forget mode**
   - Submit without waiting for ack (Ack/Reconciler is source of truth).
   - Requires impeccable reconciliation & kill-switch confidence.

---

## Execution Notes

* Tackle phases sequentially. Each completed phase should be merged/tested before
  starting the next.
* Keep configuration-driven constants (`config.toml`) for ladder offsets, cooldowns,
  kill thresholds to avoid recompiles during tuning.
* Update documentation (`docs/`) as subsystems land so operational runbooks stay current.
* Ensure CI/Dry-run tests cover new modules (`cargo check --example mm_avellaneda`,
  targeted unit tests for metrics & gates).

