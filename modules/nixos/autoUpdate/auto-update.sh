#!/bin/bash
tempfile=$(mktemp)
chmod 777 "$tempfile"
complete=0

log() {
    echo "$1" |& tee -a "$tempfile"
}
if [ -n "$HEALTHCHECK_UUID" ] && [ -n "$HEALTHCHECK_URL" ]; then
    if ! curl --silent --show-error --retry 5 "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID/start"; then
        log "Failed to start healthcheck ping uuid: $HEALTHCHECK_UUID"
    fi
fi

exit() {
    if [ -n "$HEALTHCHECK_UUID" ] && [ -n "$HEALTHCHECK_URL" ]; then
        if [ "$complete" -eq 0 ]; then
            curl --silent --show-error --retry 5 --data-raw "$(cat "$tempfile")" "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID/fail"
        else
            curl --silent --show-error --retry 5 "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID"
        fi
    fi
    trap - EXIT
}

trap exit EXIT

log "setting git safe directory"

git config --global --add safe.directory "$CONFIG_PATH"

log "Switching to $CONFIG_PATH"

cd "$CONFIG_PATH" || exit 1

if ! git config --local --get filter.git-crypt.smudge > /dev/null;
then
    log "Locked and must be unlocked before update"
    exit
fi

hashBefore=$(git rev-parse HEAD)

sudo -u "$USER" bash <<EOF
git pull --rebase |& tee -a "$tempfile"
if [ "${PIPESTATUS[0]}" -ne 0 ]; then
    log "Failed to pull changes"
    exit 1
fi
EOF
PULL_EXIT_CODE=$?
echo "PULL_EXIT_CODE: $PULL_EXIT_CODE"
if [[ $PULL_EXIT_CODE -ne 0 ]]; then
    log "Failed to pull"
    exit
fi
hashAfter=$(git rev-parse HEAD)

if [ "$hashBefore" == "$hashAfter" ]; then
    log "No changes"
    complete=1
    exit
fi

nixos-rebuild switch --flake ".#$(hostname)" |& tee -a "$tempfile"
if [ "${PIPESTATUS[0]}" -ne 0 ]; then
    log "Failed to rebuild"
    exit
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
   [ "$current_system_kernel_modules" != "$booted_system_kernel_modules" ];
then
    log "Reboot required"
    if [ "$REBOOT" == "no" ] || [ "$REBOOT" == "n" ]  || [ "$REBOOT" == "false" ] || [ "$REBOOT" == "0" ]; then
        log "Reboot skipped"
        # check if display is available
        if test -v DISPLAY; then
            notify-send -u critical "Updates Complete" "Reboot required to complete updates"
        fi
        complete=1
        exit
    else
        shutdown -r +1 "Rebooting for updates in 1 minute"
        # check if NTFY_TOPIC is set
        if [ -n "$NTFY_TOPIC" ]; then
            log "Sending notification to https://ntfy.sh/$NTFY_TOPIC"
            curl --silent --show-error --retry 5 -H "X-Title: $(hostname) rebooting in 1 min" \
            -d "$(hostname) is rebooting in 1 min as necessary for updates" \
            "https://ntfy.sh/$NTFY_TOPIC"
        fi
        if [ -n "$HEALTHCHECK_UUID" ] && [ -n "$HEALTHCHECK_URL" ]; then
            curl --silent --show-error --retry 5 "$HEALTHCHECK_URL/ping/$HEALTHCHECK_UUID/log" \
                --data-raw "Rebooting for updates in 1 minute"
        fi
    fi
fi

complete=1