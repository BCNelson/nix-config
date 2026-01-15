#!/usr/bin/env bash
set -euo pipefail

# Accept repo and PR as args, or source from resolve-pr.sh
if [[ $# -eq 2 ]]; then
  REPO="$1"
  PR_NUMBER="$2"
elif [[ $# -eq 0 ]]; then
  # Auto-resolve using sibling script
  SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  eval "$("$SCRIPT_DIR/resolve-pr.sh")"
else
  echo "Usage: fetch-unresolved-comments.sh [REPO PR_NUMBER]" >&2
  echo "       fetch-unresolved-comments.sh  (auto-detect from current branch)" >&2
  exit 1
fi

echo "# Unresolved PR Review Comments"
echo "Repository: $REPO | PR #$PR_NUMBER"
echo ""

# Fetch review threads and filter for unresolved ones
gh api graphql -f query='
query($owner: String!, $repo: String!, $pr: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $pr) {
      title
      url
      reviewThreads(first: 100) {
        nodes {
          isResolved
          isOutdated
          path
          line
          comments(first: 10) {
            nodes {
              author { login }
              body
              createdAt
            }
          }
        }
      }
    }
  }
}' -f owner="${REPO%/*}" -f repo="${REPO#*/}" -F pr="$PR_NUMBER" | \
jq -r '
.data.repository.pullRequest as $pr |
"## \($pr.title)\n\($pr.url)\n",
(.data.repository.pullRequest.reviewThreads.nodes[] |
  select(.isResolved == false) |
  "### \(.path):\(.line // "general")\n" +
  (.comments.nodes | map("**@\(.author.login)** (\(.createdAt)):\n\(.body)\n") | join("\n")) +
  "\n---\n"
)
'
