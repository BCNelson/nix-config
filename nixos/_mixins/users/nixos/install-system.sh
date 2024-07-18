#!/usr/bin/env bash
TARGET_HOST="${1:-}"
TARGET_USER="${2:-bcnelson}"
TARGET_DISK="${3:-}"

if [ "$(id -u)" -eq 0 ]; then
    echo "ERROR! $(basename "$0") should be run as a regular user"
    exit 1
fi

if [ ! -d "$HOME/nix-config/.git" ]; then
    git clone https://github.com/bcnelson/nix-config.git "$HOME/nix-config"
fi

echo "Changeing directory to $HOME/nix-config"
pushd "$HOME/nix-config" || exit 1

echo "Decrypting Repository"
gpg --decrypt local.key.asc | git-crypt unlock -

if [[ -z "$TARGET_HOST" ]]; then
    echo "ERROR! $(basename "$0") requires a hostname as the first argument"
    exit 1
fi

TAGET_HOST_PREFIX=$(echo "$TARGET_HOST" | cut -d'-' -f1)

if [[ -z "$TARGET_USER" ]]; then
    echo "ERROR! $(basename "$0") requires a username as the second argument"
    exit 1
fi

if [[ -z "$TARGET_DISK" ]]; then
    echo "ERROR! $(basename "$0") requires a disk as the third argument"
    exit 1
fi

echo "WARNING! The disk $TARGET_DISK in $TAGET_HOST_PREFIX is about to get wiped"
echo "         NixOS will be re-installed"
echo "         This is a destructive operation"
echo
read -p "Are you sure? [y/N]" -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    sudo true

    # Check if the target host has a disks.nix file.
    disk_nix=""
    if [ -f "nixos/$TAGET_HOST_PREFIX/disks.nix" ]; then
        # If so, use it to formate the disks.
        echo "Using nixos/$TAGET_HOST_PREFIX/disks.nix"
        disk_nix="nixos/$TAGET_HOST_PREFIX/disks.nix"
    else
        # Otherwise, use the default disks.nix.
        echo "Using disko/default.nix"
        disk_nix="disko/default.nix"
    fi

    sudo nix run github:nix-community/disko \
        --extra-experimental-features "nix-command flakes" \
        --no-write-lock-file \
        -- \
        --mode zap_create_mount \
        "$disk_nix" \
        --arg disk "\"$TARGET_DISK\""

    sudo nixos-generate-config --dir "nixos/$TAGET_HOST_PREFIX" --root /mnt

    sudo rm -f "./nixos/$TAGET_HOST_PREFIX/configuration.nix"

    if [ ! -f "./nixos/$TAGET_HOST_PREFIX/default.nix" ]; then
        echo "writing default.nix:"
        # This is a hack to get around bash not handling muilt line strings well.
        echo "eyAuLi4gfToKewogIGltcG9ydHMgPSBbCiAgICAuL2hhcmR3YXJlLWNvbmZpZ3VyYXRpb24ubml4CiAgXTsKfQ== | base64 -d > ./nixos/$TAGET_HOST_PREFIX/default.nix"
    fi

    awk -v TARGET_HOST=$TARGET_HOST -v TARGET_USER=$TARGET_USER '
{
    rep = sprintf("\&\n        \"%s\" = libx.mkHost { hostname = \"%s\"; usernames = [ \"%s\" ]; inherit libx; version = \"unstable\"; };", TARGET_HOST, TARGET_HOST, TARGET_USER);
    sub(/INSERT_HOST_CONFIG/, rep, $0)
}1
' flake.nix >> flake.nix.tmp
    mv flake.nix.tmp flake.nix

    git add -A
    git config user.email "admin@nel.family"
    git config user.name "Automated Installer"
    git commit -m "Install $TAGET_HOST_PREFIX"
    

    # remove user config
    git config --unset user.email
    git config --unset user.name

    sudo nixos-install --no-root-password --flake ".#$TARGET_HOST"

    # Rsync nix-config to the target install
    echo "Rsyncing $HOME to /mnt/home/$TARGET_USER"
    sudo rsync -a  "$HOME/nix-config" "/config"
    sudo rsync -a --delete "$HOME/nix-config" "/mnt/home/$TARGET_USER"
    pushd "/mnt/home/$TARGET_USER/nix-config" || exit 1
    popd || exit 1

    # Set the users password to expire on first login.
    # There is a missing feature in sddm that prevents login if the password is expired.
    # the user will need to login via the console and change their password.
    sudo nixos-enter -c "passwd --expire $TARGET_USER"
fi