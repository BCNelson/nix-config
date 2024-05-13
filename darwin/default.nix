{ outputs, hostname, usernames, lib, ... }:
let
  # Get the hostname prefix from the hostname (e.g. sierria in sierria-1)
  hostnamePrefix = lib.strings.concatStrings (lib.lists.take 1 (lib.strings.splitString "-" hostname));
in
{
  imports = lib.optional (builtins.pathExists ./${hostnamePrefix}) ./${hostnamePrefix}
    ++ builtins.filter builtins.pathExists (map (username: ./users/${username}.nix) usernames);

  services.nix-daemon.enable = true;

  nixpkgs = {
    overlays = [
      # outputs.overlays.additions
      outputs.overlays.modifications
      outputs.overlays.unstable-packages
    ];
    config = {
      allowUnfree = true;
    };
  };
}
