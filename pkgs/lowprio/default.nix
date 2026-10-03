{ lib, writeShellApplication, writeText, bashInteractive, coreutils, gnugrep, systemd, util-linux }:
let
  # lowprio COMMAND [ARGS...]
  #
  # Runs COMMAND in its own transient scope under lowprio.slice (defined in
  # home-manager/bcnelson/_mixins/lowprio.nix), at nice 10 and the lowest
  # best-effort IO priority. The scope is what keeps a build away from the
  # agent that launched it: cgroup weights decide between cgroups, and nice
  # only ranks processes inside one. It is also the unit systemd-oomd kills,
  # so a runaway build dies alone instead of taking the herdr server with it.
  #
  # Best-effort rather than idle IO, and a CPU weight rather than CPUWeight=
  # idle on the slice: an agent is usually blocked on the command, so it must
  # make progress even while the desktop is busy.
  #
  # Falls back to plain nice/ionice when the user bus is unreachable -- notably
  # inside Codex's sandbox, which refuses the socket connect. The bus is probed
  # first rather than retrying on failure, because systemd-run's exit status
  # cannot tell "no scope" apart from "the command failed", and running the
  # command twice is worse than running it unscoped.
  #
  # Tools are called by absolute path rather than through runtimeInputs: this
  # execs into the caller's command, which must see the caller's PATH, not one
  # with these store paths prepended.
  lowprio = writeShellApplication {
    name = "lowprio";
    text = ''
      if [ $# -eq 0 ]; then
        echo "usage: lowprio COMMAND [ARGS...]" >&2
        exit 2
      fi

      # Already demoted -- an agent nested in another agent's command, or a
      # wrapped shell calling lowprio again. One scope and one nice step is
      # enough; stacking them would only push nice to 19.
      if ${gnugrep}/bin/grep -q '/lowprio\.slice/' /proc/self/cgroup 2>/dev/null; then
        exec "$@"
      fi

      prio=(${coreutils}/bin/nice -n 10 ${util-linux}/bin/ionice -c 2 -n 7)

      if ${systemd}/bin/busctl --user --quiet --timeout=2 call org.freedesktop.systemd1 /org/freedesktop/systemd1 \
        org.freedesktop.DBus.Peer Ping >/dev/null 2>&1; then
        exec ${systemd}/bin/systemd-run --user --scope --quiet --collect --slice=lowprio.slice \
          --description="lowprio: $1" -- "''${prio[@]}" "$@"
      fi

      exec "''${prio[@]}" "$@"
    '';
  };

  # A `bash` that sends `-c` invocations through lowprio and passes anything
  # else (an interactive or login shell) straight through. For agents whose
  # only hook is "which shell runs my commands" -- opencode's `shell` and pi's
  # `shellPath`. It must be named bash: opencode picks its quoting and login
  # handling from the shell's basename, and refuses fish/nu outright.
  #
  # Never put this on PATH -- it would shadow the real bash for everything.
  agentShell = writeShellApplication {
    name = "bash";
    text = ''
      if [ "''${1:-}" = "-c" ]; then
        exec ${lib.getExe lowprio} ${lib.getExe bashInteractive} "$@"
      fi
      exec ${lib.getExe bashInteractive} "$@"
    '';
  };

  # For Claude Code's CLAUDE_CODE_SHELL_PREFIX. Claude hands the prefix one
  # argument -- a whole bash script -- and puts it in front of everything it
  # launches: Bash tool calls, but also hooks and stdio MCP servers. Only the
  # tool calls should be demoted: a hook blocks the agent while it runs, and an
  # MCP server is long-lived and would become an oomd kill candidate. Tool
  # calls are the ones that open by sourcing a shell snapshot.
  claudePrefix = writeShellApplication {
    name = "claude-shell-prefix";
    text = ''
      if [ $# -ne 1 ]; then
        exec "$@"
      fi
      case "$1" in
        *"/shell-snapshots/snapshot-"*)
          exec ${lib.getExe lowprio} ${lib.getExe bashInteractive} -c "$1"
          ;;
      esac
      exec ${lib.getExe bashInteractive} -c "$1"
    '';
  };

  # For Codex, via shell_environment_policy.set.BASH_ENV. Codex has no shell
  # override (it takes the passwd login shell, run as `bash -lc`), and a
  # PreToolUse rewrite would change the command text its prefix_rule
  # allow-list matches against, so `go test` and friends would start
  # prompting. BASH_ENV instead runs inside the command's own shell before the
  # command, leaving the text untouched.
  #
  # The shell moves itself into a lowprio scope when it can reach the user bus
  # -- commands approved to run outside the sandbox -- and otherwise (inside
  # Codex's sandbox, which refuses the socket) just lowers its own priority.
  # Either way every child inherits it. StartTransientUnit returns before the
  # move lands, so wait briefly for it: anything forked before then would stay
  # behind in the agent's cgroup.
  #
  # Guarded by an exported variable so nested bash scripts do not renice again.
  bashEnv = writeText "lowprio-bash-env" ''
    if [ -z "''${LOWPRIO_ENTERED:-}" ]; then
      export LOWPRIO_ENTERED=1
      if ! ${gnugrep}/bin/grep -q '/lowprio\.slice/' /proc/$$/cgroup 2>/dev/null; then
        if ${systemd}/bin/busctl --user --quiet --timeout=2 call org.freedesktop.systemd1 \
          /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager StartTransientUnit \
          'ssa(sv)a(sa(sv))' "lowprio-$$-$RANDOM.scope" fail \
          3 PIDs au 1 $$ Slice s lowprio.slice CollectMode s inactive-or-failed 0 >/dev/null 2>&1; then
          for _ in 1 2 3 4 5 6 7 8 9 10; do
            ${gnugrep}/bin/grep -q '/lowprio\.slice/' /proc/$$/cgroup && break
            ${coreutils}/bin/sleep 0.005
          done
        fi
        ${util-linux}/bin/renice -n 10 -p $$ >/dev/null 2>&1
        ${util-linux}/bin/ionice -c 2 -n 7 -p $$ >/dev/null 2>&1
      fi
    fi
  '';
in
lowprio.overrideAttrs (old: {
  passthru = (old.passthru or { }) // { inherit agentShell claudePrefix bashEnv; };
})
