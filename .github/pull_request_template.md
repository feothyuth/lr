## Summary
<!-- Short summary of the change (one or two sentences) -->

## Implementation notes
- Files changed:
- Key design choices:
- Performance constraints / tradeoffs:

## Tests
- Unit tests added: (list)
- Integration tests added: (list)
- Manual test steps (if any):

## Checklist (required)
- [ ] `cargo fmt --all` passed
- [ ] `cargo clippy --workspace -- -D warnings` passed
- [ ] Unit tests pass locally
- [ ] CI checks passing
- [ ] Code owners reviewed (crypto/orders)

## Risks / Rollout
- Risk level: High / Medium / Low
- Feature flag available? (yes/no)
- Any DB / config / infra changes required?

## Reviewer guidance
- Areas to focus on: nonce handling, signing, rounding/decimals, hot-path allocations, panic guards
- If changes affect trading/crypto logic, require at least 1 human reviewer with expertise.
