{ config, pkgs, lib, inputs, libx, ... }:
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

  environment.systemPackages = [
    pkgs.pinentry-curses
    # Ship disko rather than letting install-system `nix run` it from GitHub.
    # That fetch resolves against disko's *own* nixpkgs, not ours, so nothing
    # already on this ISO can be reused and it pulls a fresh stdenv, gcc,
    # systemd and util-linux into the live store -- which is a tmpfs. On a 2 GB
    # thin client that store is ~1 GB and the download fills it, so the install
    # dies with "No space left on device" before it has partitioned anything.
    # Taken from our own pinned input, so it shares the ISO's closure.
    inputs.disko.packages.${pkgs.stdenv.hostPlatform.system}.disko
  ];

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
  # installer does before disko creates swap -- cloning the repo, running
  # agenix-rekey -- lands in the live image's tmpfs-backed store overlay.
  # Compressed swap in RAM is what keeps that from ending at the OOM killer, and
  # it is also what makes the enlarged store below safe. Costs nothing on a
  # machine with plenty.
  zramSwap = {
    enable = true;
    memoryPercent = 100;
  };

  # The writable half of the live store is a tmpfs, and iso-image.nix mounts it
  # with no size= at all -- so it gets tmpfs's default of half of RAM, i.e.
  # ~983 MB on a thin client. agenix-rekey alone can exceed that.
  #
  # tmpfs pages are swappable, so with zram above this is not a promise of 4 GB
  # of RAM: it is a promise that the store may grow until RAM plus compressed
  # swap is genuinely exhausted, rather than stopping dead at an arbitrary
  # halfway mark.
  #
  # This has to replace the whole `fileSystems` set, not one mount. Overriding
  # `fileSystems."/nix/.rw-store"` looks right and silently does nothing:
  # installation-cd-base.nix writes `fileSystems = mkImageMediaOverride
  # config.lib.isoFileSystems`, putting priority 60 on the entire option, so a
  # per-mount definition at default priority 100 is discarded before any mkForce
  # inside it is even looked at. Rebuilding the set from config.lib.isoFileSystems
  # is what that indirection exists for -- see the comment above it upstream.
  fileSystems = lib.mkForce (config.lib.isoFileSystems // {
    "/nix/.rw-store" = {
      fsType = "tmpfs";
      options = [ "mode=0755" "size=4G" ];
      neededForBoot = true;
    };
  });
}
