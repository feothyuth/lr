# CLAUDE: Implementer guidelines

This doc explains how to use Claude as the **implementer AI** for this repo.

## Role
Claude acts as the code *implementer*: it writes features, unit tests, and a minimal PR. The human or Reviewer AI then audits and approves.

## Hard constraints for Claude runs
- Branch: create `feature/<ticket>-short-desc`.
- Keep PR under **300 lines** changed when possible.
- Add at least one unit test for non-trivial logic.
- **No new runtime dependency** without explicit human approval.
- Always run (and fix) `cargo fmt` and `cargo clippy` before committing.
  - `cargo fmt --all`
  - `cargo clippy --workspace -- -D warnings`
- Do not change `src/crypto/*` or `src/orders/*` without explicit human approval.

## Deliverables (per task)
- Implementation files changed (list)
- Unit tests added
- Short PR description (3-4 lines)
- Commit message follows: `feat: <short description> (#<issue>)`

## Example prompt to run Claude (copy/paste)

```
You are CLAUDE and you will implement one well-scoped feature in https://github.com/feothyuth/lr.

Task: <one-sentence description>

Constraints:
- Branch: feature/<ticket>-short-desc
- Keep PR < 300 changed lines
- Add unit tests. No new runtime deps
- Run `cargo fmt` and `cargo clippy` (no warnings allowed)
- Do not touch src/crypto or src/orders unless explicitly permitted

Files to modify/create:
- <path/to/file.rs>
- <path/to/tests.rs>

Unit tests to add (explicit):
- test_name_1: assert X
- test_name_2: assert Y

Produce:
1. Full patch ready for commit (diff or files)
2. 3-line test strategy
3. Suggested PR title & body
```

## Common mistakes to avoid
- Changing multiple logical concerns in one PR
- Skipping tests or using integration tests as a substitute for unit tests
- Using floats where fixed-point integers are required in money logic

## When Claude should stop and ask for human input
- Any change in `src/crypto` or anything that moves funds
- Any dependency bump for crypto, networking, or serialization crates
- Changes that change economic logic (fees, rounding, inventory caps)
