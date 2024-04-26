{ pkgs, ... }:

{
  imports =
    [
      # Include the results of the hardware scan.
      ../_mixins/roles/tailscale.nix
    ];

  # If ephemeral is true, then tailscale will be removed on next reboot
  systemd.services.tailscaled = {
    serviceConfig.Environment = [ "FLAGS=--state=mem: --tun 'tailscale0'" ];
  };

  environment.systemPackages = with pkgs; [
    pinentry
    pinentry-qt
  ];

  programs.gnupg.agent = {
    enable = true;
    pinentryFlavor = "qt";
  };
}
