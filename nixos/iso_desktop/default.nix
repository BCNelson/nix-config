{ pkgs, lib, ... }:

{
  imports =
    [
      # Include the results of the hardware scan.
      ../_mixins/roles/tailscale.nix
      # Same eMMC gap as the console ISO: this one installs to whatever hardware
      # it is pointed at, and small-form-factor machines boot off eMMC too.
      ../_mixins/hardware/emmc.nix
    ];

  # If ephemeral is true, then tailscale will be removed on next reboot
  systemd.services.tailscaled = {
    serviceConfig.Environment = [ "FLAGS=--state=mem: --tun 'tailscale0'" ];
  };

  environment.systemPackages = with pkgs; [
    pinentry-qt
  ];

  nix.settings.substituters = lib.mkBefore [ "https://nixcache.nel.family/" ];
}
