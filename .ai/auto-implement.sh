#!/bin/bash
# Fully automated implementation
# Usage: ./auto-implement.sh "your task"

TASK="$1"
BRANCH="feature/$(echo "$TASK" | tr '[:upper:]' '[:lower:]' | tr ' ' '-' | cut -c1-30)"

# Generate prompt
PROMPT=$(cat <<EOF
Follow .github/CLAUDE.md and AGENTS.md guidelines.

Task: ${TASK}

Requirements:
- Create branch ${BRANCH}
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
)

# Check if Claude Code is available in the session
echo "📋 Task: $TASK"
echo "🌿 Branch: $BRANCH"
echo ""
echo "Prompt generated. Send this to Claude Code:"
echo "─────────────────────────────────────────"
echo "$PROMPT"
echo "─────────────────────────────────────────"
echo ""
echo "Claude Code will implement, test, commit, and push automatically."
