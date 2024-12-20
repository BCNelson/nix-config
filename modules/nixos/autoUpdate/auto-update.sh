#!/bin/bash
set -euo pipefail

tempfile=$(mktemp)
chmod 777 "$tempfile"
complete=0

log() {
    echo "$1" |& tee -a "$tempfile"
}

# Verify required variables
if [ -z "${CONFIG_PATH-}" ]; then
    log "CONFIG_PATH not set"
    exit 1
fi

if [ -z "${USER-}" ]; then
    log "USER not set"
    exit 1
fi

# Handle healthcheck file
if [ -n "${HEALTHCHECK_UUID_FILE-}" ] && [ -f "$HEALTHCHECK_UUID_FILE" ]; then
    HEALTHCHECK_UUID="$(cat "$HEALTHCHECK_UUID_FILE")"
    log "HEALTHCHECK_UUID: $HEALTHCHECK_UUID"
else
    HEALTHCHECK_UUID=""
    log "HEALTHCHECK_UUID_FILE not set or file not found"
fi

# Start healthcheck if configured
if [ -n "${HEALTHCHECK_UUID-}" ] && [ -n "${HEALTHCHECK_URL-}" ]; then
    if ! curl --silent --show-error --retry 5 "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID/start"; then
        log "Failed to start healthcheck ping uuid: $HEALTHCHECK_UUID"
    fi
fi

cleanup_and_exit() {
    if [ -n "${HEALTHCHECK_UUID-}" ] && [ -n "${HEALTHCHECK_URL-}" ]; then
        if [ "$complete" -eq 0 ]; then
            curl --silent --show-error --retry 5 --data-raw "$(cat "$tempfile")" "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID/fail"
        else
            curl --silent --show-error --retry 5 "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID"
        fi
    fi
    rm -f "$tempfile"
    trap - EXIT
    exit "${1:-0}"
}

trap 'cleanup_and_exit $?' EXIT

log "setting git safe directory"
git config --global --add safe.directory "$CONFIG_PATH"

log "Switching to $CONFIG_PATH"
cd "$CONFIG_PATH" || cleanup_and_exit 1

if ! git config --local --get filter.git-crypt.smudge > /dev/null; then
    log "Locked and must be unlocked before update"
    cleanup_and_exit 1
fi

hashBefore=$(git rev-parse HEAD) || {
    log "Failed to get current commit hash"
    cleanup_and_exit 1
}

# Pull changes
sudo -u "$USER" bash <<EOF
git pull --rebase |& tee -a "$tempfile"
if [ "\${PIPESTATUS[0]}" -ne 0 ]; then
    log "Failed to pull changes"
    exit 1
fi
EOF

PULL_EXIT_CODE=$?
log "PULL_EXIT_CODE: $PULL_EXIT_CODE"
if [[ $PULL_EXIT_CODE -ne 0 ]]; then
    log "Failed to pull"
    cleanup_and_exit 1
fi

hashAfter=$(git rev-parse HEAD) || {
    log "Failed to get new commit hash"
    cleanup_and_exit 1
}

if [ "$hashBefore" == "$hashAfter" ]; then
    log "No changes"
    complete=1
    cleanup_and_exit 0
fi

# Rebuild system
if ! nixos-rebuild switch --flake ".#$(hostname -s)" |& tee -a "$tempfile"; then
    log "Failed to rebuild"
    cleanup_and_exit 1
fi

# Check if a reboot is required
current_system_initrd=$(readlink /run/current-system/initrd)
current_system_kernel=$(readlink /run/current-system/kernel)
current_system_kernel_modules=$(readlink /run/current-system/kernel-modules)

booted_system_initrd=$(readlink /run/booted-system/initrd)
booted_system_kernel=$(readlink /run/booted-system/kernel)
booted_system_kernel_modules=$(readlink /run/booted-system/kernel-modules)

if [ "$current_system_initrd" != "$booted_system_initrd" ] ||
   [ "$current_system_kernel" != "$booted_system_kernel" ] ||
   [ "$current_system_kernel_modules" != "$booted_system_kernel_modules" ]; then
    log "Reboot required"
    if [[ "${REBOOT,,}" =~ ^(no|n|false|0)$ ]]; then
        log "Reboot skipped"
        # Check if display is available and functional
        if [ -n "${DISPLAY-}" ] && xhost >/dev/null 2>&1; then
            notify-send -u critical "Updates Complete" "Reboot required to complete updates"
        fi
        complete=1
        cleanup_and_exit 0
    else
        shutdown -r +1 "Rebooting for updates in 1 minute"
        # Send notifications if configured
        if [ -n "${NTFY_TOPIC-}" ]; then
            log "Sending notification to https://ntfy.sh/$NTFY_TOPIC"
            curl --silent --show-error --retry 5 \
                -H "X-Title: $(hostname -s) rebooting in 1 min" \
                -d "$(hostname -s) is rebooting in 1 min as necessary for updates" \
                "https://ntfy.sh/$NTFY_TOPIC"
        fi
        if [ -n "${HEALTHCHECK_UUID-}" ] && [ -n "${HEALTHCHECK_URL-}" ]; then
            curl --silent --show-error --retry 5 "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID/log" \
                --data-raw "Rebooting for updates in 1 minute"
        fi
    fi
fi

complete=1