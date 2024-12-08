{ pkgs, libx, lib, ... }:
let
  hostKey = libx.getSecret ../sensitive.nix "isoAgePrivateKey";
  hostKeyFile = pkgs.writeText "hostKey" hostKey;
in
{
  imports =
    [
      # Include the results of the hardware scan.
      ../_mixins/roles/tailscale.nix
    ];

  environment.systemPackages = [ pkgs.pinentry ];

  programs.gnupg.agent = {
    enable = true;
    pinentryPackage = pkgs.pinentry;
  };

  age.identityPaths = [ hostKeyFile ];

  nix.settings.substituters = lib.mkBefore [ "https://nixcache.nel.family/" ];

  # If ephemeral is true, then tailscale will be removed on next reboot
  systemd.services.tailscaled = {
    serviceConfig.Environment = [ "FLAGS=--state=mem: --tun 'tailscale0'" ];
  };

}
