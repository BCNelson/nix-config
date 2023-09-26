{ config, desktop, lib, pkgs, username, ... }:
let
  ifExists = groups: builtins.filter (group: builtins.hasAttr group config.users.groups) groups;
  install-system = pkgs.writeShellApplication {
    name = "install-system";
    runtimeInputs = with pkgs; [ git gnupg git-crypt ];
    text = ''
    TARGET_HOST="''${1:-}"
    TARGET_USER="''${2:-bcnelson}"
    TARGET_DISK="''${3:-}"

    if [ "$(id -u)" -eq 0 ]; then
      echo "ERROR! $(basename "$0") should be run as a regular user"
      exit 1
    fi

    if [ ! -d "$HOME/nix-config/.git" ]; then
      git clone https://github.com/bcnelson/nix-config.git "$HOME/nix-config"
    fi

    echo "Changeing directory to $HOME/nix-config"
    pushd "$HOME/nix-config"

    echo "Decrypting Repository"
    gpg --decrypt local.key.asc | git-crypt unlock -

    if [[ -z "$TARGET_HOST" ]]; then
      echo "ERROR! $(basename "$0") requires a hostname as the first argument"
      exit 1
    fi

    if [[ -z "$TARGET_USER" ]]; then
      echo "ERROR! $(basename "$0") requires a username as the second argument"
      exit 1
    fi

    if [[ -z "$TARGET_DISK" ]]; then
      echo "ERROR! $(basename "$0") requires a disk as the third argument"
      exit 1
    fi

    echo "WARNING! The disks in $TARGET_HOST are about to get wiped"
    echo "         NixOS will be re-installed"
    echo "         This is a destructive operation"
    echo
    read -p "Are you sure? [y/N]" -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
      sudo true

      # Check if the target host has a disks.nix file.
      disk_nix=""
      if [ -f "nixos/$TARGET_HOST/disks.nix" ]; then
        # If so, use it to formate the disks.
        echo "Using nixos/$TARGET_HOST/disks.nix"
        disk_nix="nixos/$TARGET_HOST/disks.nix"
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

      sudo nixos-generate-config --dir "nixos/$TARGET_HOST" --root /mnt

      rm -f "/mnt/nixos/$TARGET_HOST/configuration.nix"

      git add -A
      git commit -m "Install $TARGET_HOST"

      echo "Would you like to open a PR to merge this change?"
      read -p "Are you sure? [y/N]" -n 1 -r
      if [[ $REPLY =~ ^[Yy]$ ]]; then
        git remote add gitmask https://git.gitmask/v1/gh/bcnelson/nix-config
        git push gitmask addHost:master
      fi

      sudo nixos-install --no-root-password --flake ".#$TARGET_HOST"

      # Rsync nix-config to the target install and set the remote origin to SSH.
      echo "Rsyncing $HOME to /mnt/home/$TARGET_USER"
      rsync -a --delete "$HOME/nix-config" "/mnt/home/$TARGET_USER"
      pushd "/mnt/home/$TARGET_USER/nix-config"
      git remote set-url origin git@github.com:bcnelson/nix-config.git
      popd

      # Set the users password to expire on first login.
      # There is a missing feature in sddm that prevents login if the password is expired.
      # the user will need to login via the console and change their password.
      sudo nixos-enter -c "passwd --expire $TARGET_USER"
    fi
  '';
  };
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
    packages = [ pkgs.home-manager pkgs.libsForQt5.kate ];
  };

  config.system.stateVersion = lib.mkForce lib.trivial.release;
  config.environment.systemPackages = [ install-system ];
  config.services.kmscon.autologinUser = "${username}";
}
