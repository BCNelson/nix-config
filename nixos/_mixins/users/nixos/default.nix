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

      TAGET_HOST_PREFIX=$(echo "$TARGET_HOST" | cut -d'-' -f1)

      if [[ -z "$TARGET_USER" ]]; then
        echo "ERROR! $(basename "$0") requires a username as the second argument"
        exit 1
      fi

      if [[ -z "$TARGET_DISK" ]]; then
        echo "ERROR! $(basename "$0") requires a disk as the third argument"
        exit 1
      fi

      echo "WARNING! The disks in $TAGET_HOST_PREFIX are about to get wiped"
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

        rm -f "/mnt/nixos/$TAGET_HOST_PREFIX/configuration.nix"

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
        rsync -a --delete "$HOME/nix-config" "/mnt/home/$TARGET_USER"
        pushd "/mnt/home/$TARGET_USER/nix-config"
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
