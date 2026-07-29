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

  environment.systemPackages = [ pkgs.pinentry-curses ];

  programs.gnupg.agent = {
    enable = true;
    pinentryPackage = pkgs.pinentry-curses;
  };

  age.identityPaths = [ hostKeyFile ];

  # If ephemeral is true, then tailscale will be removed on next reboot
  systemd.services.tailscaled = {
    serviceConfig.Environment = [ "FLAGS=--state=mem: --tun 'tailscale0'" ];
  };

  # This ISO installs the thin clients, which have ~2 GB of RAM. Everything the
  # installer does before disko creates swap -- cloning the repo, `nix run`ning
  # disko itself, agenix-rekey -- lands in the live image's tmpfs-backed store
  # overlay, which is sized at half of RAM. Compressed swap in RAM is what keeps
  # that from ending at the OOM killer. Costs nothing on a machine with plenty.
  zramSwap.enable = true;
}
