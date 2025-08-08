{ ... }: {
  imports = [
    ./audiobookshelf.nix
    ./nixBinaryCacheProxy.nix
    ./romm.nix
    ./node-red.nix
    ./frigate.nix
  ];

  systemd.timers.podman-auto-update = {
    enable = true;
  };
}
