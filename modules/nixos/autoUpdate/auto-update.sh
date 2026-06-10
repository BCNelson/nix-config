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
            # Send the log as the request body via stdin, not as a CLI argument:
            # a failed nixos-rebuild log can exceed the kernel's per-argument
            # limit (~128KiB), which makes `--data-raw "$(cat ...)"` fail to exec
            # curl at all, so the failure ping never reaches the health check.
            # Cadence caps the stored body (DefaultMaxBodyBytes = 10KiB) and keeps
            # the head, so send the *tail* — the actual error is at the end.
            tail -c 10000 "$tempfile" | curl --silent --show-error --retry 5 \
                --data-binary @- "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID/fail" || true
        else
            curl --silent --show-error --retry 5 "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID" || true
        fi
    fi
    rm -f "$tempfile"
    trap - EXIT
    exit "${1:-0}"
}

trap 'cleanup_and_exit $?' EXIT

# Persistent retry state across timer-spaced runs. systemd provides
# $STATE_DIRECTORY via StateDirectory=; fall back for manual invocation.
STATE_DIR="${STATE_DIRECTORY:-/var/lib/auto-update}"
STATE_FILE="$STATE_DIR/state"
MAX_RETRIES="${MAX_RETRIES:-3}"

# BUILT_COMMIT  - last commit that built successfully
# ATTEMPT_COMMIT - commit currently being retried
# ATTEMPT_COUNT  - rebuild attempts made for ATTEMPT_COMMIT
BUILT_COMMIT=""
ATTEMPT_COMMIT=""
ATTEMPT_COUNT=0

mkdir -p "$STATE_DIR"
if [ -f "$STATE_FILE" ]; then
    # shellcheck disable=SC1090
    source "$STATE_FILE"
fi

save_state() {
    {
        echo "BUILT_COMMIT=$BUILT_COMMIT"
        echo "ATTEMPT_COMMIT=$ATTEMPT_COMMIT"
        echo "ATTEMPT_COUNT=$ATTEMPT_COUNT"
    } > "$STATE_FILE"
}

log "setting git safe directory"
git config --global --add safe.directory "$CONFIG_PATH"

log "Switching to $CONFIG_PATH"
cd "$CONFIG_PATH" || cleanup_and_exit 1

if ! git config --local --get filter.git-crypt.smudge > /dev/null; then
    log "Locked and must be unlocked before update"
    cleanup_and_exit 1
fi

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

targetCommit=$(git rev-parse HEAD) || {
    log "Failed to get new commit hash"
    cleanup_and_exit 1
}

# Decide whether to build based on the last commit we actually built, not on
# whether the pull moved HEAD. Otherwise a commit that fails to build is built
# exactly once (HEAD moves to it) and every later run sees "No changes",
# stops retrying, and falsely reports success.
if [ "$targetCommit" == "$BUILT_COMMIT" ]; then
    log "Already built $targetCommit; up to date"
    complete=1
    cleanup_and_exit 0
fi

# New commit to attempt -> reset the per-commit retry counter.
if [ "$targetCommit" != "$ATTEMPT_COMMIT" ]; then
    ATTEMPT_COMMIT="$targetCommit"
    ATTEMPT_COUNT=0
fi

# Retry limit reached: stop rebuilding this commit but keep reporting failure
# every interval until a new (fix) commit arrives.
if [ "$ATTEMPT_COUNT" -ge "$MAX_RETRIES" ]; then
    log "Commit $targetCommit failed $ATTEMPT_COUNT times; retry limit ($MAX_RETRIES) reached, not rebuilding"
    save_state
    complete=0
    cleanup_and_exit 1
fi

# Persist the incremented count BEFORE building so a timeout/crash still counts.
ATTEMPT_COUNT=$((ATTEMPT_COUNT + 1))
log "Rebuild attempt $ATTEMPT_COUNT/$MAX_RETRIES for $targetCommit"
save_state

# Rebuild system. If `switch` is blocked by switchInhibitors (e.g. dbus
# implementation or kernel changes that can't be applied live), fall back to
# `boot` so the new generation is staged for the next reboot — otherwise the
# build sits idle and the next run skips with "No changes".
rebuild_output=$(mktemp)
rebootRequired=0
if ! nixos-rebuild switch --flake ".#$(hostname -s)" |& tee -a "$tempfile" "$rebuild_output"; then
    if grep -qF "Pre-switch check 'switchInhibitors' failed" "$rebuild_output"; then
        log "switch blocked by switchInhibitors; falling back to 'nixos-rebuild boot' and scheduling reboot"
        if ! nixos-rebuild boot --flake ".#$(hostname -s)" |& tee -a "$tempfile"; then
            log "Failed to rebuild (boot fallback)"
            rm -f "$rebuild_output"
            cleanup_and_exit 1
        fi
        rebootRequired=1
    else
        log "Failed to rebuild"
        rm -f "$rebuild_output"
        cleanup_and_exit 1
    fi
fi
rm -f "$rebuild_output"

# Rebuild succeeded: record it and clear the retry counter so later runs treat
# this commit as up to date.
BUILT_COMMIT="$targetCommit"
ATTEMPT_COMMIT=""
ATTEMPT_COUNT=0
save_state

# Check if a reboot is required
current_system_initrd=$(readlink /run/current-system/initrd)
current_system_kernel=$(readlink /run/current-system/kernel)
current_system_kernel_modules=$(readlink /run/current-system/kernel-modules)

booted_system_initrd=$(readlink /run/booted-system/initrd)
booted_system_kernel=$(readlink /run/booted-system/kernel)
booted_system_kernel_modules=$(readlink /run/booted-system/kernel-modules)

if [ "$rebootRequired" -eq 1 ] ||
   [ "$current_system_initrd" != "$booted_system_initrd" ] ||
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