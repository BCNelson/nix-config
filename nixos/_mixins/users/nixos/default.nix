{ config, desktop, lib, pkgs, username, ... }:
let
  ifExists = groups: builtins.filter (group: builtins.hasAttr group config.users.groups) groups;
  install-system = pkgs.writeScriptBin "install-system" ''
    #!${pkgs.stdenv.shell}

    #set -euo pipefail

    TARGET_HOST="''${1:-}"
    TARGET_USER="''${2:-bcnelson}"

    if [ "$(id -u)" -eq 0 ]; then
      echo "ERROR! $(basename "$0") should be run as a regular user"
      exit 1
    fi

    if [ ! -d "$HOME/nix-config/.git" ]; then
      git clone https://github.com/bcnelson/nix-config.git "$HOME/nix-config"
    fi

    pushd "$HOME/nix-config"

    if [[ -z "$TARGET_HOST" ]]; then
      echo "ERROR! $(basename "$0") requires a hostname as the first argument"
      echo "       The following hosts are available"
      ls -1 nixos/*/boot.nix | cut -d'/' -f2 | grep -v iso
      exit 1
    fi

    if [[ -z "$TARGET_USER" ]]; then
      echo "ERROR! $(basename "$0") requires a username as the second argument"
      echo "       The following users are available"
      ls -1 nixos/_mixins/users/ | grep -v -E "nixos|root"
      exit 1
    fi

    # Check if the machine we're provisioning expects a keyfile to unlock a disk.
    # If it does, generate a new key, and write to a known location.
    if grep -q "data.keyfile" "nixos/$TARGET_HOST/disks.nix"; then
      echo -n "$(head -c32 /dev/random | base64)" > /tmp/data.keyfile
    fi

    echo "WARNING! The disks in $TARGET_HOST are about to get wiped"
    echo "         NixOS will be re-installed"
    echo "         This is a destructive operation"
    echo
    read -p "Are you sure? [y/N]" -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
      sudo true

      sudo nixos-install --no-root-password --flake ".#$TARGET_HOST"

      # Rsync nix-config to the target install and set the remote origin to SSH.
      rsync -a --delete "$HOME/Zero/" "/mnt/home/$TARGET_USER/Zero/"
      pushd "/mnt/home/$TARGET_USER/nix-config"
      git remote set-url origin git@github.com:bcnelson/nix-config.git
      popd

      # If there is a keyfile for a data disk, put copy it to the root partition and
      # ensure the permissions are set appropriately.
      if [[ -f "/tmp/data.keyfile" ]]; then
        sudo cp /tmp/data.keyfile /mnt/etc/data.keyfile
        sudo chmod 0400 /mnt/etc/data.keyfile
      fi
    fi
  '';
in
{
  # Only include desktop components if one is supplied.
  imports = [ ] ++ lib.optional (builtins.isString desktop) ./desktop.nix;

  config.users.users.nixos = {
    description = "NixOS";
    extraGroups = [
      "audio"
      "networkmanager"
      "users"
      "video"
      "wheel"
    ]
    ++ ifExists [
      "docker"
      "podman"
    ];
    homeMode = "0755";
    packages = [ pkgs.home-manager ];
  };

  config.system.stateVersion = lib.mkForce lib.trivial.release;
  config.environment.systemPackages = [ install-system ];
  config.services.kmscon.autologinUser = "${username}";
}
