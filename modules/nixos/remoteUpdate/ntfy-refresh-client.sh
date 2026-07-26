#!/usr/bin/env bash
# Subscribe to the refresh topic so a push schedules a check, instead of the
# poll timer having to be short enough to feel responsive.
echo "Starting remote-update ntfy client"
NTFY_REFRESH_TOPIC="$(cat "$NTFY_REFRESH_TOPIC_FILE")"
exec ntfy subscribe "$NTFY_REFRESH_TOPIC" "$REFRESH_COMMAND"
