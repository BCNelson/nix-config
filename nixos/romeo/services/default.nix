{pkgs, ...}: let
  cadencePingExec = slug:
    pkgs.writeShellScript "cadence-ping-${slug}" ''
      exec ${pkgs.curl}/bin/curl -fsS -m 10 --retry 2 --retry-delay 2 \
        "https://health.b.nel.family/ping/$(cat /run/agenix/cadence_check_${builtins.replaceStrings ["-"] ["_"] slug})"
    '';
in {
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
    ./frigate.nix
    ./gotosocial.nix
    ./ollama.nix
    ./open-webui.nix
    ./jellyswarrm.nix
    ./immich.nix
    ./immichframe.nix
    ./journiv.nix
    ./tendant.nix
    ./woodpecker-agent.nix
  ];

  systemd.timers.podman-auto-update = {
    enable = true;
    wantedBy = ["multi-user.target"];
  };

  systemd.services.podman-auto-update.serviceConfig.ExecStartPost = [
    "+${cadencePingExec "podman-auto-update-romeo"}"
  ];

  age.secrets.cadence_check_podman_auto_update_romeo.rekeyFile =
    ../../../secrets/store/cadence/checks/podman-auto-update-romeo.age;
}
