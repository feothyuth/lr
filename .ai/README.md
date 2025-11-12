# AI Workflow Scripts

The shortest way to use the full AI workflow.

## Usage

### 1. Implement a feature (Claude)

```bash
.ai/implement.sh "add position tracking"
```

Copy the output and paste it to Claude.

### 2. Review the changes (Codex/GPT)

```bash
.ai/review.sh feature-branch-name
```

Copy the output and paste it to Codex/GPT for review.

## That's it!

The scripts automatically:
- Reference all constraints from .github/CLAUDE.md
- Generate proper branch names
- Include git diffs for review
- Follow the full workflow

**Shortest possible:**
1. Run implement script → paste to Claude
2. Run review script → paste to Codex
3. Merge if approved
