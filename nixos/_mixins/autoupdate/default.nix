{ pkg, ... }:
let
    updateScript = pkgs.writeShellApplication {
        name = "auto-update";
        runtimeInputs = with pkgs; [ git gnupg git-crypt coreutils ];
        text = builtins.readFile ./auto-update.sh;
    };
in
{
    systemd.timers.auto-update = {
        enable = true;
        timerConfig = {
            OnBootSec="15min";
            OnUnitActiveSec="1w";
            Persistent = true;
        };
    };

    systemd.services.auto-update = {
        enable = true;
        serviceConfig = {
            Type = "oneshot";
            User = "root";
            ExecStart = "${updateScript}/bin/auto-update.sh";
        };
    };
}