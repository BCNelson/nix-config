{ lib, pkgs, ... }:

# A BUSY Bar on the desk showing how many coding agents are asking for you,
# working and done, as herdr-core (./hc.nix) sees them -- see pkgs/herdr-busybar for
# both halves. It reads the core's socket and never starts a core itself.
#
# Only for a machine the bar is plugged into over USB (redo-3). The bar's USB
# network is a fixed point-to-point link, bar 10.0.4.20 and this host
# 10.0.4.21, so the addresses are the package defaults rather than options.
#
# The bridge runs whenever the user session does, docked or not: it binds its
# address before the link exists (IP_FREEBIND), installs/updates the bar's app
# when the bar appears, and launches it after every dock or bar reboot. While
# undocked it only fails a 3-second status check every 5 seconds.

let
  inherit (pkgs) herdr-busybar;
in
{
  systemd.user.services.herdr-busybar = {
    Unit.Description = "herdr-core agent counts for the BUSY Bar";
    Service = {
      ExecStart = lib.concatStringsSep " " [
        (lib.getExe herdr-busybar)
        "-app"
        "${herdr-busybar}/${herdr-busybar.appPackage}"
      ];
      Restart = "always";
      RestartSec = 5;
    };
    Install.WantedBy = [ "default.target" ];
  };
}
