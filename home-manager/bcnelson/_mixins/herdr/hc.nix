{ config, lib, outputs, pkgs, ... }:

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
  systemctl = "${pkgs.systemd}/bin/systemctl";

  # The default session's core belongs to herdr-core.service (below), not to
  # whichever `hc` call finds it down: that call would start a transient
  # hc-serve-* core with the caller's environment, and an `hc server restart`
  # would leave the unit inactive with a transient core in its place. So,
  # for the default session only (no HC_SESSION/HC_SOCKET of its own):
  #
  # - no socket: start the unit and give it a moment to bind, before hc
  #   looks. hc would otherwise start its own core at once; that one loses
  #   the core.lock flock and exits, but it is noise and a wasted spawn.
  # - `hc server restart`: restart the unit instead. Stopping the unit is a
  #   SIGTERM, which herdr-core treats as a planned restart (handover to the
  #   holders, programs keep running), the same as the RPC hc would send.
  #
  # `hc server stop` stays hc's own: a real stop (kept terminals parked) whose
  # clean exit the unit doesn't restart; the next hc call starts it again.
  # Without a user manager (no systemctl answer) everything falls through to
  # hc's own autostart, as before.
  hcUnit = ''
    if [ -z "''${HC_SESSION:-}" ] && [ -z "''${HC_SOCKET:-}" ] && [ -n "''${XDG_RUNTIME_DIR:-}" ]; then
      sock="$XDG_RUNTIME_DIR/herdr-core/default.sock"
      if [ "$#" -eq 2 ] && [ "$1" = server ] && [ "$2" = restart ]; then
        if ${systemctl} --user restart herdr-core.service 2>/dev/null; then
          set -- server status
        fi
      elif [ ! -S "$sock" ]; then
        ${systemctl} --user start herdr-core.service 2>/dev/null || true
      fi
      for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
        [ -S "$sock" ] && break
        ${pkgs.coreutils}/bin/sleep 0.1
      done
    fi
  '';

  herdr-core = pkgs.symlinkJoin {
    name = "herdr-core-clean-env-${pkgs.herdr-core.version}";
    paths = [ pkgs.herdr-core ];
    nativeBuildInputs = [ pkgs.makeWrapper ];
    # Only hc gets hcUnit: herdr-core.service runs the herdr-core wrapper,
    # which must not try to start its own unit.
    postBuild = ''
      wrapProgram $out/bin/herdr-core --run ${lib.escapeShellArg cleanEnv}
      wrapProgram $out/bin/hc --run ${lib.escapeShellArg cleanEnv} --run ${lib.escapeShellArg hcUnit}
    '';
    inherit (pkgs.herdr-core) meta;
  };

  # herdr-core orders its own units for shutdown: holders Before, cores After
  # a guard unit, hc-shutdown-<FNV-1a 64 of the session's state dir>.service
  # (platform/systemd.rs shutdown_guard), so a core saves its screens before
  # the shells under the holders exit. A core run by our unit needs the same
  # After=. The name is computed at build time into a drop-in (Nix integers
  # error on overflow, so not in Nix itself). If upstream ever changes how it
  # names the guard, this silently orders against nothing -- check with
  # `systemctl --user list-units 'hc-shutdown-*'` after a herdr-core bump.
  stateDir = "${config.xdg.stateHome}/herdr-core/default";
  shutdownGuardDropIn = pkgs.runCommand "herdr-core-shutdown-guard.conf" { nativeBuildInputs = [ pkgs.python3 ]; } ''
    python3 -c '
    import sys
    h = 0xcbf29ce484222325
    for b in sys.argv[1].encode():
        h = ((h ^ b) * 0x100000001b3) & (2**64 - 1)
    print("[Unit]\nAfter=hc-shutdown-%016x.service" % h)
    ' ${lib.escapeShellArg stateDir} > $out
  '';

  # Any transient core still around (from before this unit, or one an `hc`
  # outside the wrapper started) holds core.lock, and ours would lose the
  # flock and exit. Stop it first: SIGTERM, so it hands its programs over
  # rather than ending them, and our core adopts them. Only the default
  # session is used on these machines; a transient core for another
  # --session would be stopped too (and restart on its next hc call).
  stopTransientCores = pkgs.writeShellScript "herdr-core-stop-transient" ''
    ${systemctl} --user stop 'hc-serve-*.service' || true
  '';

  # The core's environment, the same whenever it starts. A unit otherwise gets
  # the user manager's environment as it is at that moment: at boot (linger)
  # just environment.d, but after a desktop login also everything the session
  # imported -- KDE/GTK settings, SESSION_MANAGER, GPG_TTY=/dev/tty2,
  # __HM_SESS_VARS_SOURCED=1 (so fish skips home-manager's session vars) --
  # and every terminal inherits the core's. So start from nothing and add:
  #
  # - environment.d as systemd's own generator expands it from an empty
  #   environment: what the manager has at boot, before any login touches it
  #   (XDG_*_HOME, TERMINFO_DIRS, LOCALE_ARCHIVE, the system PATH).
  # - who and where: HOME, USER, LOGNAME, XDG_RUNTIME_DIR, and the locale,
  #   which the manager takes from locale.conf at its own start.
  # - PATH: environment.d's, then a fixed fallback (NixOS sets the manager's
  #   PATH outside environment.d). The herdr-core wrapper puts
  #   /run/wrappers/bin first; fish's config adds the Nix profiles.
  # - SHELL=fish, the shell terminals run (config.toml above).
  #
  # The login session's variables (DISPLAY, WAYLAND_DISPLAY, SSH_AUTH_SOCK,
  # DBUS_SESSION_BUS_ADDRESS, XDG_SESSION_*, XDG_CURRENT_DESKTOP, ...) need
  # nothing here: core re-reads them from the manager for every terminal it
  # opens (platform/systemd.rs apply_session). What the desktop session adds
  # beyond those and the login-shell files (KDE/GTK settings,
  # SESSION_MANAGER, ...) is deliberately not inherited.
  #
  # A package rather than a store path in ExecStart, so the unit file (and with
  # it the restart trigger) doesn't change on every bash or systemd bump.
  # Changes to this script take effect on the next core restart.
  herdrCoreServe = pkgs.writeShellApplication {
    name = "herdr-core-serve";
    text = ''
      vars=()
      while IFS= read -r line; do
        case "$line" in *=*) vars+=("$line") ;; esac
      done < <(${pkgs.coreutils}/bin/env -i HOME="$HOME" \
        ${pkgs.systemd}/lib/systemd/user-environment-generators/30-systemd-environment-d-generator)

      path=
      for v in "''${vars[@]}"; do
        case "$v" in PATH=*) path=''${v#PATH=} ;; esac
      done
      fallback="/run/current-system/sw/bin:${config.home.profileDirectory}/bin:/etc/profiles/per-user/$USER/bin:/nix/var/nix/profiles/default/bin:/usr/local/bin:/usr/bin:/bin"

      keep=()
      for k in HOME USER LOGNAME XDG_RUNTIME_DIR LANG LANGUAGE LC_ALL LC_CTYPE LC_MESSAGES LC_TIME LC_NUMERIC LC_COLLATE LC_MONETARY LC_PAPER LC_MEASUREMENT TZ; do
        if [ -n "''${!k:-}" ]; then keep+=("$k=''${!k}"); fi
      done

      # The ssh agent, when it is systemd's socket-activated one (Fedora's
      # ssh-agent.socket on redo-3). Core's per-terminal re-read from the
      # manager only finds SSH_AUTH_SOCK once a desktop login has imported it
      # (Plasma's env/ssh-agent.sh), so the terminals a boot-time core
      # restores before that came up without an agent. The socket is
      # listening from sockets.target on, before this unit starts; once the
      # session does export it, the re-read hands terminals the same path.
      if [ -n "''${XDG_RUNTIME_DIR:-}" ] && [ -S "$XDG_RUNTIME_DIR/ssh-agent.socket" ]; then
        keep+=("SSH_AUTH_SOCK=$XDG_RUNTIME_DIR/ssh-agent.socket")
      fi

      # Then through a login shell, so the core also gets what /etc/profile,
      # /etc/profile.d and ~/.profile add for a desktop login (Fedora's
      # DOCKER_HOST and SSH_ASKPASS, NixOS's set-environment with its PATH,
      # home-manager's session vars). Those are files, not the moment, so
      # the result is still the same at boot and after login. Cleared after:
      # what only describes the login shell itself (TERM=dumb would stick:
      # core only fills in TERM when it is missing; GPG_TTY="not a tty"), and
      # the "already sourced" guards, so each terminal's fish re-reads
      # home-manager's session vars and picks up a switch without a core
      # restart.
      # shellcheck disable=SC2016 # expanded by the login shell, not here
      exec ${pkgs.coreutils}/bin/env -i "''${vars[@]}" "''${keep[@]}" \
        PATH="''${path:+$path:}$fallback" \
        ${pkgs.bash}/bin/bash -lc '
          unset TERM GPG_TTY PWD OLDPWD SHLVL _ __HM_SESS_VARS_SOURCED __ETC_PROFILE_NIX_SOURCED
          export SHELL="$1"
          exec "$0" serve
        ' "${config.home.profileDirectory}/bin/herdr-core" "${config.home.profileDirectory}/bin/fish" </dev/null
    '';
  };
in
{
  # Structured agent sessions for the Android app and herdr-app mods, beside
  # this machine's core. See the module for what it does and why it is a user
  # service.
  imports = [ outputs.homeModules.herdr-agent-parser ];
  services.herdr-agent-parser.enable = true;

  home.packages = [
    herdr-core
    herdrCoreServe
    pkgs.hc-tailscale
  ];

  # New terminals otherwise get $SHELL, as handed to whichever process first
  # started the core -- bash on redo-3, whose login shell belongs to Fedora.
  # fish matches the herdr TUI (terminal.default_shell in ./core.nix), by
  # absolute path for the same reason: the core's PATH is whatever its
  # starter had. Core reads this file fresh for every terminal and never
  # writes it, so a read-only link is fine and needs no core restart.
  xdg.configFile."herdr-core/config.toml".source = (pkgs.formats.toml { }).generate "herdr-core-config.toml" {
    shell = [ "${pkgs.fish}/bin/fish" ];
  };

  # The default session's core as a fixed unit, so its environment is the
  # same every time and a herdr-core update restarts it.
  #
  # Environment: herdr-core-serve's fixed one (above), not whoever ran `hc`
  # first nor whatever the manager has accumulated by the time it starts.
  #
  # Restarts: stop is a SIGTERM, a planned restart -- programs keep running in
  # their hc-hold-* units and the next core adopts them. home-manager
  # (sd-switch) restarts the unit when its file changes. ExecStart goes
  # through the profile, whose paths never change, and X-Restart-Triggers
  # names the unwrapped package, so it restarts for a herdr-core update but
  # not for every bash or fish bump that would rebuild the wrapper.
  #
  # The rest mirrors the transient units hc itself creates
  # (platform/systemd.rs unit_args): KillMode=process, so stopping the core
  # takes nothing else along; OOMPolicy=continue; stderr to core.log in the
  # state dir, where hc's error messages point.
  systemd.user.services.herdr-core = {
    Unit = {
      Description = "herdr-core (hc), default session";
      X-Restart-Triggers = [ "${pkgs.herdr-core}" ];
    };
    Service = {
      Type = "exec";
      ExecStartPre = "-${stopTransientCores}";
      ExecStart = "${config.home.profileDirectory}/bin/herdr-core-serve";
      WorkingDirectory = "%h";
      StateDirectory = "herdr-core/default";
      StateDirectoryMode = "0700";
      StandardOutput = "null";
      StandardError = "append:${stateDir}/core.log";
      KillMode = "process";
      OOMPolicy = "continue";
      # A clean exit is `hc server stop`, which should stay stopped.
      Restart = "on-failure";
      RestartSec = 2;
    };
    Install.WantedBy = [ "default.target" ];
  };

  xdg.configFile."systemd/user/herdr-core.service.d/10-shutdown-guard.conf".source = shutdownGuardDropIn;

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
