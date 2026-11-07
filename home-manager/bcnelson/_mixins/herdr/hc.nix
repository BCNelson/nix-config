{ lib, pkgs, ... }:

# herdr-core (`hc`) and a Tailscale transport for reaching other machines from
# it.
#
# `hc machine add --via CMD` swaps ssh for any command that runs a shell line
# on the far end and passes bytes both ways untouched. pkgs/hc-tailscale is
# that command for a tailnet this machine's own tailscaled is NOT on (a work
# laptop's system Tailscale belongs to the work tailnet): it embeds a userspace
# tsnet node and talks Tailscale SSH over it, so nothing about the system
# tailscaled changes.
#
# A "slot" is one such node, with its own login and therefore its own tailnet.
# The default slot is `default`; `--slot work` is another login. One slot
# reaches every machine on its tailnet -- the target is per call:
#
#   hc-tailscale login                     # once per slot; prints a URL
#   hc machine add --via 'hc-tailscale ssh bcnelson@romeo' --name romeo
#   hc-tailscale --slot work login
#   hc machine add --via 'hc-tailscale --slot work ssh me@devbox' --name devbox
#   hc-tailscale slots                     # what is logged in where
#
# Login is its own step because --via must never prompt; an unauthenticated
# slot, or a Tailscale SSH "check" policy wanting a browser, fails the --via
# call at once with the URL or command to run instead of hanging hc.
#
# The nodes live in a user daemon rather than in each --via call. hc runs its
# --via command many times, sometimes concurrently; two processes booting tsnet
# from one state dir share a node key and knock each other off, and each would
# pay a cold control/DERP start. The daemon brings up every slot on disk at
# start and creates new ones on `login`.
#
# The devices show up as <machine>-hc[-<slot>] owned by whoever logged in, so
# the tailnet's SSH policy treats them like that user's other devices.
{
  home.packages = [
    pkgs.herdr-core
    pkgs.hc-tailscale
  ];

  systemd.user.services.hc-tailscale = {
    Unit.Description = "Tailscale (tsnet) transport for herdr-core";
    Service = {
      ExecStart = "${lib.getExe pkgs.hc-tailscale} serve";
      # ~/.local/state/hc-tailscale/<slot>/, handed over as STATE_DIRECTORY.
      StateDirectory = "hc-tailscale";
      StateDirectoryMode = "0700";
      # tsnet uploads its backend logs to log.tailscale.com by default; they
      # stay in the journal instead.
      Environment = [ "TS_NO_LOGS_NO_SUPPORT=true" ];
      Restart = "on-failure";
      RestartSec = 5;
    };
    Install.WantedBy = [ "default.target" ];
  };
}
