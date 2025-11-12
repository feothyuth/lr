#!/bin/bash
# Usage: ./review.sh <branch-name>

BRANCH="${1:-HEAD}"

cat <<EOF
You are the code reviewer. Review this diff following AGENTS.md guidelines.

$(git diff main...$BRANCH)

Check for:
- Nonce handling, signing logic, rounding/decimals
- Missing tests and edge cases
- Hot-path allocations and panics

Provide:
- Verdict: APPROVE / MINOR CHANGES / REQUEST CHANGES
- List of issues with file:line
- Suggested patches
EOF
