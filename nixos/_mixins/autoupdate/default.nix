{ pkgs, libx, ... }:
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
      OnUnitActiveSec = "5m";
      Persistent = true;
    };
    wantedBy = [ "timers.target" ];
  };

  systemd.services.auto-update = {
    enable = true;
    environment = {
      NTFY_TOPIC = ntfy_topic;
    };
    serviceConfig = {
      Type = "oneshot";
      User = "root";
      ExecStart = "${updateScript}/bin/auto-update";
    };
    restartIfChanged = false;
  };

  systemd.services.startup-notify = {
    enable = true;
    wantedBy = [ "multi-user.target" ];
    wants = [ "network-online.target" ];
    serviceConfig = {
      Type = "oneshot";
    };
    script = with pkgs; ''
      #!/usr/bin/env bash
      ${curl}/bin/curl -H "X-Title: $HOSTNAME has Started" \
          -H "X-Priority: 1" \
          -d "$HOSTNAME Has Booted!" \
          https://ntfy.sh/${ntfy_topic}
    '';
  };
}
