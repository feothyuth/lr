# Contributing

Thank you for contributing! This document describes the repo workflow, CI expectations, and AI-assisted roles.

## Branch and PR rules
- Branch naming: `feature/<ticket>-short-desc`, `fix/<ticket>-short-desc`, `chore/<short-desc>`
- Keep PRs small and focused (aim ≤ 300 lines).
- Every non-trivial change must include at least one unit test or integration test.
- No dependency bumps without explicit approval for core runtime crates.

## Roles & AI workflow
- **Implementer AI** (choose one model to act consistently: `Claude` or `Codex`):
  - Writes the implementation, adds unit tests, runs `cargo fmt` and `cargo clippy`.
  - Pushes a feature branch and opens the PR.
- **Reviewer AI** (the other model):
  - Consumes the PR diff and produces an inline review with suggested patches and missing tests.
- **Human**:
  - Final reviewer for critical areas (crypto, signing, funds movement).
  - Required approver for code under `src/crypto` and `src/orders`.

## Local checks (must pass before opening PR)
```bash
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo test --workspace
# optional: security
cargo install --version 0.16.0 cargo-audit || true
cargo audit || true
```

## CI requirements

CI must pass for merge:

* fmt-check
* clippy
* unit-tests
* cargo-audit (optional failure policy configurable by team)

## PR template

Use `.github/pull_request_template.md` (automatically applied by GitHub).

## Code owners

Add `CODEOWNERS` in `.github/` to require reviews for sensitive paths.

## How to request the AI implementer

Provide:

* Exact files to touch
* Strict performance & dependency constraints
* Test cases to assert
* Max allowed lines changed

## Safety note

AI outputs should be treated as helpers; **always run tests locally** and do a human review for crypto/trading code prior to merging.
