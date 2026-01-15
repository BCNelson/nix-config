#!/usr/bin/env bash
set -euo pipefail

PR_NUMBER="${1:-}"

# Get repo info
REPO=$(gh repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null)
if [[ -z "$REPO" ]]; then
  echo "Error: Could not determine repository. Are you in a git repo with a GitHub remote?" >&2
  exit 1
fi

# Auto-detect PR if not provided
if [[ -z "$PR_NUMBER" ]]; then
  BRANCH=$(git branch --show-current 2>/dev/null)
  if [[ -z "$BRANCH" ]]; then
    echo "Error: Not in a git repository or detached HEAD" >&2
    exit 1
  fi

  PR_NUMBER=$(gh pr view "$BRANCH" --json number -q '.number' 2>/dev/null || true)

  if [[ -z "$PR_NUMBER" ]]; then
    echo "Error: No open PR found for branch '$BRANCH'" >&2
    echo "Usage: resolve-pr.sh [PR_NUMBER]" >&2
    exit 1
  fi

  echo "Branch: $BRANCH" >&2
fi

# Output in parseable format
echo "REPO=$REPO"
echo "PR_NUMBER=$PR_NUMBER"
