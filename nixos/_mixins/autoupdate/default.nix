{ pkgs, libx, healthcheckUuid, ... }:
let
  updateScript = pkgs.writeShellApplication {
    name = "auto-update";
    runtimeInputs = with pkgs; [ git gnupg git-crypt coreutils just bash nix nixos-rebuild systemd curl hostname ];
    text = builtins.readFile ./auto-update.sh;
  };
  ntfy_topic = libx.getSecret ../../sensitive.nix "ntfy_topic";
in
{
  systemd.timers.auto-update = {
    enable = true;
    timerConfig = {
      OnBootSec = "1min";
      OnUnitActiveSec = "10m";
      Persistent = true;
    };
    wantedBy = [ "timers.target" ];
  };

  systemd.services.auto-update = {
    enable = true;
    environment = {
      NTFY_TOPIC = ntfy_topic;
      HEALTHCHECK_UUID = healthcheckUuid;
    };
    serviceConfig = {
      Type = "oneshot";
      User = "root";
      ExecStart = "${updateScript}/bin/auto-update";
      TimeoutStartSec = "6h";
    };
    restartIfChanged = false;
  };

  systemd.services.startup-notify = {
    enable = true;
    wantedBy = [ "multi-user.target" ];
    wants = [ "network-online.target" ];
    serviceConfig = {
      Type = "simple";
      RemainAfterExit = true;
    };
    script = with pkgs; ''
      #!/usr/bin/env bash
      ${curl}/bin/curl -H "X-Title: $HOSTNAME has Started" \
          -H "X-Priority: 2" \
          -d "$HOSTNAME Has Booted!" \
          https://ntfy.sh/${ntfy_topic}
    '';
    restartIfChanged = false;
  };
}
