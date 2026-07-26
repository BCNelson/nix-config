#!/usr/bin/env bash
# Republish on demand instead of waiting out the publish timer: subscribe to
# the same refresh topic autoUpdate uses, so a push reaches the thin clients
# within their poll interval rather than up to a full timer period later.
echo "Starting closure-publisher ntfy client"
NTFY_REFRESH_TOPIC="$(cat "$NTFY_REFRESH_TOPIC_FILE")"
exec ntfy subscribe "$NTFY_REFRESH_TOPIC" "$REFRESH_COMMAND"
