# AI Workflow Scripts

The shortest way to use the full AI workflow with ANY AI (Claude, Codex, GPT, etc.)

## Usage

### 1. Implement a feature (use ANY AI)

```bash
.ai/task.sh "add position tracking"
```

Copy the output and paste it to **any AI** (Claude, Codex, GPT, etc.)

### 2. Review the changes (use ANY AI)

```bash
.ai/review-any.sh feature-branch-name
```

Copy the output and paste it to **any AI** for review.

## That's it!

The scripts automatically:
- Reference all constraints from .github/CLAUDE.md
- Generate proper branch names
- Include git diffs for review
- Follow the full workflow
- Work with ANY AI (no fixed roles)

**Shortest possible:**
1. Run task script → paste to any AI
2. Run review script → paste to any AI
3. Merge if approved

**No fixed roles** - use whichever AI you want for implementation or review!
