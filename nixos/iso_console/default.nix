{ pkgs, libx, ... }:
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

  age.identityPaths = [ hostKeyFile ];

  # If ephemeral is true, then tailscale will be removed on next reboot
  systemd.services.tailscaled = {
    serviceConfig.Environment = [ "FLAGS=--state=mem: --tun 'tailscale0'" ];
  };

}
