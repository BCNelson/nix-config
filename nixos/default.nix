# Edit this configuration file to define what should be installed on
# your system.  Help is available in the configuration.nix(5) man page
# and in the NixOS manual (accessible by running ‘nixos-help’).

{ outputs, hostname, usernames, desktop, thinClient ? false, lib, stateVersion, ... }:

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
    ++ lib.optional (builtins.isString desktop) ./_mixins/roles/desktop
    # Membership of hosts/thin-clients.nix is what makes a host a thin client.
    # Pulling the role in from here rather than from the host's own default.nix
    # means the registry cannot disagree with what a host actually imports --
    # previously a hostname could be listed but not import the role, and romeo
    # would happily build and publish closures the host never polled for.
    ++ lib.optional thinClient ./_mixins/roles/thin-client;

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
}
