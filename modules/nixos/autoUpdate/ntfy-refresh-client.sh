#!/usr/bin/env bash
echo "Starting ntfy-refresh-client"
NTFY_REFRESH_TOPIC="$(cat "$NTFY_REFRESH_TOPIC_FILE")"
# shellcheck disable=SC2016
ntfy subscribe "$NTFY_REFRESH_TOPIC" 'echo "Starting Update"; systemctl start auto-update.service'