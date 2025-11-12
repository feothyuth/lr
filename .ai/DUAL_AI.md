# Dual AI Workflow (Claude + Codex)

Work with **Claude Code** and **Codex** at the same time.

## Setup

### Claude Code (Implementation)
You're already using me in this terminal. I handle:
- Implementation
- Testing
- Committing
- Pushing

### Codex (Review)
Open a separate window/tab for Codex to review.

---

## Workflow

### Step 1: Tell me to implement
**In this Claude Code session:**
```
Implement: add exponential backoff to WebSocket reconnection
```

I will:
1. Create branch
2. Write code + tests
3. Commit and push
4. Output: "✅ Pushed to branch feature/websocket-backoff"

### Step 2: Get review from Codex
**Run this command:**
```bash
.ai/review-any.sh feature/websocket-backoff
```

**Copy the output and paste to Codex** in a separate window/tab.

### Step 3: Codex reviews
Codex will provide:
- Verdict (APPROVE/CHANGES NEEDED)
- List of issues
- Suggested fixes

### Step 4: Tell me to apply fixes
**Back in Claude Code:**
```
Apply Codex's suggestions: [paste Codex's feedback]
```

I will:
1. Apply the fixes
2. Run tests
3. Commit and push

### Step 5: Merge
Either:
- Tell me: "Merge feature/websocket-backoff"
- Or merge manually via GitHub UI

---

## Quick Reference

| Task | Tool | Command |
|------|------|---------|
| Implement | Claude Code | "Implement: [task]" |
| Review | Codex | `.ai/review-any.sh [branch]` → paste output to Codex |
| Apply fixes | Claude Code | "Apply Codex's suggestions: [feedback]" |
| Merge | Claude Code or GitHub | "Merge [branch]" or use GitHub UI |

---

## Example Session

```bash
# You → Claude Code
You: Implement: add position tracking

# Claude Code does everything
Me: ✅ Pushed to branch feature/position-tracking

# You → Terminal
$ .ai/review-any.sh feature/position-tracking
[copy output]

# You → Codex (separate window)
[paste output]

# Codex responds with review
Codex: MINOR CHANGES - Missing test for edge case X at line 45

# You → Claude Code
You: Apply Codex's suggestions: Missing test for edge case X at line 45

# Claude Code fixes it
Me: ✅ Fixed and pushed

# You → Claude Code
You: Merge feature/position-tracking

# Done!
```

---

## Benefits

- **Claude Code**: Fast implementation, automatic git operations
- **Codex**: Independent review, catches issues Claude might miss
- **Both running**: Parallel workflow, faster iteration

No context switching needed - both AIs work for you at the same time!
