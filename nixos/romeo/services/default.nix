{ ... }: {
  imports = [
    ./audiobookshelf.nix
    ./nixBinaryCacheProxy.nix
    ./romm.nix
  ];

  systemd.timers.podman-auto-update = {
    enable = true;
  };
}
