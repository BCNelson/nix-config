#!/bin/sh

cd /config || exit 1

if ! git config --local --get filter.git-crypt.smudge > /dev/null;
then
    echo "Locked and must be unlocked before update"
    exit 1
fi

just --unstable sync

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
    echo "Reboot required"
    shutdown -r +2 "Rebooting for updates in 2 minutes"
    # check if NTFY_TOPIC is set
    if [ -n "$NTFY_TOPIC" ]; then
        echo "Sending notification to https://ntfy.sh/$NTFY_TOPIC"
        curl -H "X-Title: $(hostname) rebooting in 2 mins" \
          -d "$(hostname) is rebooting in 2 mins as necessary for updates" \
          "https://ntfy.sh/$NTFY_TOPIC"
    fi
fi