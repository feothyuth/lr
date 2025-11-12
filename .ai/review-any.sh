#!/bin/bash
# Usage: ./review-any.sh <branch-name>
# Works with ANY AI (Claude, Codex, GPT, etc.)

BRANCH="${1:-HEAD}"

cat <<EOF
Review this code change following AGENTS.md guidelines.

$(git diff main...$BRANCH)

Check for:
- Nonce handling, signing logic, rounding/decimals
- Missing tests and edge cases
- Hot-path allocations and panics
- Code quality and best practices

Provide:
- Verdict: APPROVE / MINOR CHANGES / REQUEST CHANGES
- List of issues with file:line
- Suggested fixes
EOF
