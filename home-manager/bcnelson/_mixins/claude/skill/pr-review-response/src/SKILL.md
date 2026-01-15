---
name: pr-respond
description: Draft brief, professional responses to GitHub PR review comments. Use /pr-respond with a PR number to fetch unresolved comments and craft replies.
allowed-tools:
  - Bash
  - Read
  - Grep
  - Glob
---

# PR Review Response

Draft brief, professional responses to PR review comments.

## Workflow

1. Run the fetch script (auto-detects PR from current branch, or accepts PR number):
   ```bash
   ./fetch-unresolved-comments.sh [PR_NUMBER]
   ```
2. Read relevant code files for context
3. Draft responses for each comment

## Response Style

**Human reviewers:** Brief and professional. 1-2 sentences max.
- "Fixed." / "Good catch, fixed."
- "Done, switched to [approach]."
- "Intentional - [one line reason]."
- "Can you clarify what you mean by X?"

**AI bot reviewers** (CodeRabbit, Copilot, etc.):
- Fixing: Ultra-brief - "Fixed." / "Done."
- Not fixing: Brief explanation - "Won't fix - [reason]" or "Intentional - [reason]"
- Nitpicks: Skip or "N/A"

## Output Format

```
**[file:line]** (@reviewer)
> [their comment summary]

[your response]

---
```

## Task Management

For PRs with multiple review comments:
1. Use TodoWrite to create a task for each unresolved comment thread
2. Mark tasks as in_progress when drafting each response
3. Mark completed after drafting (user will copy/post responses)

Example todo structure:
- "Respond to @reviewer on src/file.ts:42"
- "Respond to @coderabbit on src/utils.ts:15"

## Sub-Agents

Use Task tool with Explore agent when you need to:
- Understand unfamiliar code referenced in review comments
- Find related implementations to justify design decisions
- Search for patterns/conventions used elsewhere in the codebase

Keep exploration focused - only spawn agents when context from a simple file read isn't sufficient.

## Guidelines

- Keep responses short - respect reviewer's time
- AI bots get minimal acknowledgment unless the suggestion is substantive
- If disagreeing, one sentence of reasoning is enough
- Don't over-explain
