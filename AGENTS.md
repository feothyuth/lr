# AGENTS: AI-assisted dev workflow

This document describes the multi-agent workflow: which agent does what, how to prompt them, and the automation hooks.

## Roles
- **Implementer** — Claude (writes code + tests)
- **Reviewer** — Codex (reviews diffs, proposes precise inline fixes)
- **Human** — final sign-off for merges, required for sensitive files

## Files to add under this repo
- `CLAUDE.md` (implementer instructions)
- `AGENTS.md` (this file) — cross-links to PR template & CONTRIBUTING

## Work loop (repeatable)
1. Create small task/issue.
2. Run Implementer agent (Claude) with the standard prompt (see below).
3. Implementer pushes branch and opens PR (use template).
4. Run Reviewer agent (Codex) on PR diff; reviewer posts inline suggestions and a short verdict (APPROVE / MINOR CHANGES / REQUEST CHANGES).
5. Human runs tests locally, validates economic/crypto logic, and merges if OK.

## Reviewer prompt (Codex)

```
You are the reviewer. Analyze this PR diff: <paste diff>

Goals:
- Check nonce, signing, rounding, decimals, and hot loop allocations
- Find missing tests and edge cases
- Provide inline code suggestions and test names

Output:
- Verdict: APPROVE / MINOR CHANGES / REQUEST CHANGES
- Bullet list of issues with file:line
- Suggested small patches (applyable diffs)
```

## CI & automation hooks
- When PR opens: run CI (fmt/clippy/tests/cargo-audit)
- Optionally, run an automated AI draft review comment (non-blocking) that summarizes risk and missing tests
- Do **not** auto-merge AI-reviewed PRs — require a human approval for `src/crypto` and `src/orders`

## Prompts & templates
- Keep single canonical prompts in `CLAUDE.md` and reviewer prompts in `AGENTS.md`.
- Use the project's PR template to force checklist completion.

## Example mapping of agents -> github events
- `pull_request.opened` → run CI + auto AI draft review (comment)
- `pull_request.synchronize` → re-run CI, re-run AI draft review
- `pull_request.ready_for_review` → human + reviewer AI review

## Ops notes
- Store AI prompts as versioned files (e.g., `.ai/prompts/implementer_claude.txt`) so they can be audited.
- Keep `CODEOWNERS` to force human reviews on critical paths.

## Minimal set of files to boot the workflow
- `.github/pull_request_template.md` (PR checklist)
- `CONTRIBUTING.md`
- `.github/CODEOWNERS`
- `.github/workflows/ci.yml`
- `CLAUDE.md` & `AGENTS.md` (this file)
