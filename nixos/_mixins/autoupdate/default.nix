{ pkgs, ... }:
let
  updateScript = pkgs.writeShellApplication {
    name = "auto-update";
    runtimeInputs = with pkgs; [ git gnupg git-crypt coreutils just bash nix nixos-rebuild systemd curl ];
    text = builtins.readFile ./auto-update.sh;
  };
  sensitive = import ../../sensitive.nix;
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
      NTFY_TOPIC = sensitive.ntfy_topic;
    };
    serviceConfig = {
      Type = "oneshot";
      User = "root";
      ExecStart = "${updateScript}/bin/auto-update";
    };
    restartIfChanged = false;
  };
}
