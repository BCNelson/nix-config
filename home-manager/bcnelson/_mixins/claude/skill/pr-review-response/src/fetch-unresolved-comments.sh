#!/usr/bin/env bash
set -euo pipefail

# Let glab resolve the current branch, explicit IID/URL, and --repo override.
# Use the returned target project and host, including for fork merge requests.
if [[ "${1:-}" == "--gitlab" ]]; then
  shift
  MR=$(glab mr view "$@" --output json)
  PROJECT_ID=$(jq -er '.project_id | numbers' <<<"$MR")
  MR_IID=$(jq -er '.iid | numbers' <<<"$MR")
  MR_URL=$(jq -er '.web_url | strings' <<<"$MR")
  GITLAB_HOST=$(jq -er '.web_url | split("/")[2] | select(length > 0)' <<<"$MR")

  echo '# Unresolved Merge Request Discussions'
  jq -r '"## \(.title)\n\(.web_url)\n"' <<<"$MR"
  glab api --hostname "$GITLAB_HOST" --paginate \
    "projects/$PROJECT_ID/merge_requests/$MR_IID/discussions?per_page=100" |
    jq -r --arg url "$MR_URL" '
      .[] |
      select(any(.notes[]; .resolvable == true and .resolved == false)) |
      . as $discussion |
      [.notes[] | select(.system != true)] as $notes |
      $notes[0] as $first |
      "### \($first.position.new_path // $first.position.old_path // "general"):\($first.position.new_line // $first.position.old_line // "general")\n" +
      "Discussion: \($discussion.id)\n\($url)#note_\($first.id)\n\n" +
      ($notes | map("**@\(.author.username)** (\(.created_at)):\n\(.body)\n") | join("\n")) +
      "\n---\n"
    '
  exit 0
fi

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
  echo "       fetch-unresolved-comments.sh --gitlab [MR_IID_OR_URL] [--repo HOST/GROUP/PROJECT]" >&2
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
