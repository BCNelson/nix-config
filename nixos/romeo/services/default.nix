{ ... }: {
  imports = [
    ./actual.nix
    ./audiobookshelf.nix
    ./fastenhealth.nix
    ./foundryvtt.nix
    ./homebox.nix
    ./jellyfin.nix
    ./mealie.nix
    ./nixBinaryCacheProxy.nix
    ./romm.nix
    ./syncthing.nix
    ./node-red.nix
    ./frigate.nix
    ./ollama.nix
    ./open-webui.nix
    ./jellyswarrm.nix
    ./immich.nix
    ./immichframe.nix
  ];

  systemd.timers.podman-auto-update = {
    enable = true;
    wantedBy = ["multi-user.target"];
  };
}
