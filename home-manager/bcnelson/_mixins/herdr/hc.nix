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
let
  # Whichever `hc` call first finds no core running starts it (systemd-run
  # --user hc-serve-*), handing it the caller's environment, and the core
  # passes that on to every terminal it opens. When that caller is an agent's
  # shell, the core carries CLAUDE_CODE_CHILD_SESSION and friends forever, and
  # every `claude` started in the desktop app then believes it is a
  # sub-session and stops saving transcripts. Strip CLAUDE* on the way in;
  # herdr-core reads none of them. The app finds `hc` on PATH before its
  # built-in store path, so this covers the cores it starts too. The wrapper
  # execs the real binary, so /proc/self/exe and its build id are unchanged.
  #
  # PATH gets the same treatment. The desktop app prepends its own guesses
  # (~/.local/bin, /opt/homebrew/bin, /run/current-system/sw/bin, ...) before
  # running `hc`, which lands /run/current-system/sw/bin ahead of
  # /run/wrappers/bin. Every terminal then finds the plain, non-setuid sudo
  # from systemPackages first and fails with "must be owned by uid 0 and have
  # the setuid bit set". The app's code can't be changed, so the core is
  # handed /run/wrappers/bin first again here, as a NixOS login shell has it.
  cleanEnv = ''
    for v in ''${!CLAUDE@}; do unset "$v"; done
    if [ -d /run/wrappers/bin ]; then
      p=":$PATH:"
      while [[ $p == *:/run/wrappers/bin:* ]]; do p=''${p//:\/run\/wrappers\/bin:/:}; done
      p=''${p#:}; p=''${p%:}
      export PATH="/run/wrappers/bin''${p:+:$p}"
    fi
  '';
  herdr-core = pkgs.symlinkJoin {
    name = "herdr-core-clean-env-${pkgs.herdr-core.version}";
    paths = [ pkgs.herdr-core ];
    nativeBuildInputs = [ pkgs.makeWrapper ];
    postBuild = ''
      for b in hc herdr-core; do
        wrapProgram $out/bin/$b --run ${lib.escapeShellArg cleanEnv}
      done
    '';
    inherit (pkgs.herdr-core) meta;
  };
in
{
  home.packages = [
    herdr-core
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
