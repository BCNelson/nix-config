{ ... }: {
  imports = [
    ./audiobookshelf.nix
    ./nixBinaryCacheProxy.nix
    ./romm.nix
    ./node-red.nix
    ./frigate.nix
    ./ollama.nix
  ];

  systemd.timers.podman-auto-update = {
    enable = true;
    wantedBy = ["multi-user.target"];
  };
}
