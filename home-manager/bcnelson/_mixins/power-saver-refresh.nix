{ pkgs, ... }:
let
  power-saver-refresh = pkgs.writeShellApplication {
    name = "power-saver-refresh";
    runtimeInputs = [ pkgs.jq ];
    # Long-running daemon: keep nounset/pipefail but drop errexit so a transient
    # kscreen-doctor/gdbus hiccup doesn't tear the whole watcher down.
    bashOptions = [ "nounset" "pipefail" ];
    text = builtins.readFile ./power-saver-refresh.sh;
  };
in
{
  systemd.user.services.power-saver-refresh = {
    Unit = {
      Description = "Lower the internal panel refresh rate on the power-saver profile";
      PartOf = [ "graphical-session.target" ];
      After = [ "graphical-session.target" ];
    };

    Service = {
      ExecStart = "${power-saver-refresh}/bin/power-saver-refresh";
      Restart = "on-failure";
      RestartSec = 5;
    };

    Install = {
      WantedBy = [ "graphical-session.target" ];
    };
  };
}
