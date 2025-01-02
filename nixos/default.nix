# Edit this configuration file to define what should be installed on
# your system.  Help is available in the configuration.nix(5) man page
# and in the NixOS manual (accessible by running ‘nixos-help’).

{ outputs, hostname, usernames, desktop, lib, stateVersion, ... }:

let
  # Get the hostname prefix from the hostname (e.g. sierria in sierria-1)
  hostnamePrefix = lib.strings.concatStrings (lib.lists.take 1 (lib.strings.splitString "-" hostname));
  # Get the host postfix from the hostname (e.g. 1 in sierria-1)
  hostnamePostfix = lib.strings.concatStrings (lib.lists.drop 1 (lib.strings.splitString "-" hostname));
in
{
  imports = [ ./common.nix ./secrets.nix ]
    # ++ lib.optional common ./common.nix # Common configuration but ones that can be turned off
    ++ lib.optional (builtins.pathExists ./${hostnamePrefix}) ./${hostnamePrefix}
    ++ lib.optional (builtins.pathExists ./${hostnamePrefix}/${hostnamePostfix}.hardware-configuration.nix) ./${hostnamePrefix}/${hostnamePostfix}.hardware-configuration.nix
    ++ builtins.filter builtins.pathExists (map (username: ./_mixins/users/${username}) usernames)
    ++ lib.optional (builtins.isString desktop) ./_mixins/roles/desktop;

  networking.hostName = hostname;
  system.stateVersion = stateVersion;
  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux"; #Orverride if nessary
  nixpkgs = {
    overlays = [
      # outputs.overlays.additions
      outputs.overlays.modifications
      outputs.overlays.unstable-packages
      outputs.overlays.additions
    ];
    config = {
      allowUnfree = true;
    };
  };
  catppuccin.enable = true;
  catppuccin.sddm.enable = false;
}
