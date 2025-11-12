#!/bin/bash
# Usage: ./task.sh "your task description"
# Works with ANY AI (Claude, Codex, GPT, etc.)

TASK="$1"

cat <<EOF
Follow .github/CLAUDE.md and AGENTS.md guidelines.

Task: ${TASK}

Requirements:
- Create branch feature/$(echo "$TASK" | tr '[:upper:]' '[:lower:]' | tr ' ' '-' | cut -c1-30)
- Modify relevant files
- Add unit tests
- Run cargo fmt --all && cargo clippy --workspace -- -D warnings
- Keep PR < 300 lines
- No new dependencies without approval

After implementation:
1. Show git diff
2. Provide commit message
3. Provide PR title and body
EOF
