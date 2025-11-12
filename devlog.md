  Phase A – Observability / Instrumentation (Remaining)

  - [ ] Build rolling markout dashboards (per-side 1 s median, window 200 fills).
  - [ ] Track same-side fills/sec (95th percentile) and flip-rate.
  - [ ] Maintain inventory-age timers and net P&L per $1 M over 30–60 min.
  - [ ] Wire metrics into structured logs/CSV for external monitoring.
  - [ ] Implement automatic kill-switch triggers driven by those metrics.

  Phase C – Microstructure Gates (Enhancements)

  - [ ] Flip gate: log & expose its triggers; consider adaptive widening rather than skip-only path.
  - [ ] Age-based partial exit: integrate depth-aware sizing and confirm ladder interaction.
  - [ ] Tune ladder offsets/size multipliers via config, add back-off hysteresis.

  Phase E – Execution Pipeline Refactor (Major Work)

  - [ ] Introduce PendingOrder/LiveOrder maps keyed by client order id / order index.
  - [ ] Move to minimal diffs: generate cancel-one/replace ops instead of blanket cancel-all.
  - [ ] Background reconciliation task that consumes update/transaction and account feeds to update
    pending state.
  - [ ] Redesign ExecutionEngine:
      - async tasks for market decisions, order building, tx submission, ack reconciliation (bounded
        channels).
      - strict separation of signing and send (e.g., SignerWorker with a queue).
  - [ ] Implement per-order client IDs and order versioning.
  - [ ] Build failure backoff (retry budgets, error classification) for the new pipeline.

  Phase F – Speed Polish (Depends on E)

  - [ ] Pre-signed payload cache for ±N tick templates with background refresh.
  - [ ] Parallel order slots (A/B) to amortize ack latency; only cancel stale slot when new one is
    live.
  - [ ] Fire-and-forget / optimistic mode with reconciliation-only confirmation (make configurable).
  - [ ] Post-only + queue-protect logic for incremental modify, guard against accidental crosses.

  Housekeeping

  - [ ] Integrate participation telemetry into external monitoring (Grafana/Prometheus).
  - [ ] Extend config docs/runbooks to cover new async architecture and kill-switch behavior.
  - [ ] Add targeted unit/integration tests for participation controller, metrics aggregation, and
    pipeline components.
    
## 2025-02-14

- Replaced the synchronous Avellaneda execution layer with an asynchronous pipeline that tracks pending/live orders, performs diff-based cancel/place, and manages retries/backoff against nonce failures (`src/avellaneda/execution.rs`).
- Added dedicated worker tasks for decision intake, signing, submission, account reconciliation, and transaction ack handling, driven by bounded channels to keep the strategy loop lightweight.
- Updated the Avellaneda example to feed decisions plus account/transaction WebSocket events into the new engine APIs (`examples/trading/avellaneda/main.rs`).
- Verified builds with `cargo check` (passes; existing `OrderBookState::from_delta` dead-code warning remains) and formatted touched files via `rustfmt --edition 2021` to avoid the known `cargo fmt` issue in another example.
