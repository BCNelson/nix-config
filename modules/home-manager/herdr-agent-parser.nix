# herdr-agent-parser: structured agent sessions beside herdr-core (`hc`).
#
# Built from git.bcnelson.dev/bcnelson/herdr-android (the flake input of that
# name; its overlay lands in overlays/default.nix `additions`). The daemon
# tails coding agents' transcripts and serves one agent-neutral model -- the
# conversation, tool calls, questions, todos -- on its own socket, so the
# Android app (over `ssh ... herdr-agent-parser connect`) and herdr-app mods
# can show and answer them instead of scraping the screen. Clients that find
# it missing keep working as plain terminals.
#
# A user service, not a system one: herdr-core is per user, and the daemon
# finds core's socket under $XDG_RUNTIME_DIR/herdr-core/<session>.sock and
# listens on $XDG_RUNTIME_DIR/herdr-agent-parser/<session>.sock. It needs no
# ordering against core: core is started on demand by whichever `hc` call
# first finds it down (a transient hc-serve-* scope, not a unit we could order
# after), and the daemon retries core's socket every 2s on its own, picking
# core back up across restarts.
#
# One daemon per core session (HC_SESSION). Only `default` is used here.
#
# Import with `outputs.homeModules.herdr-agent-parser` (see ./default.nix).
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.herdr-agent-parser;
  exe = lib.getExe cfg.package;

  # The same seven events and timeout `herdr-agent-parser hook-config` prints,
  # with an absolute path rather than a bare name: Claude Code runs hooks
  # through a non-login shell, so the profile is not guaranteed on PATH. The
  # hook is silent and always exits 0, and outside an hc terminal (no
  # HC_TERMINAL_ID) it returns before reading stdin, so wiring it into every
  # Claude session costs one short-lived process per event.
  hookEvents = [
    "SessionStart"
    "UserPromptSubmit"
    "PreToolUse"
    "PostToolUse"
    "PermissionRequest"
    "Notification"
    "Stop"
  ];
  hookEntry = [
    {
      matcher = "*";
      hooks = [
        {
          type = "command";
          command = "${exe} hook";
          timeout = 5;
        }
      ];
    }
  ];

  # "default" keeps the plain unit name; other sessions get a suffix.
  unitName = session:
    if session == "default"
    then "herdr-agent-parser"
    else "herdr-agent-parser-${session}";
in {
  options.services.herdr-agent-parser = {
    enable = lib.mkEnableOption "herdr-agent-parser, structured agent sessions for herdr-core clients";

    package = lib.mkPackageOption pkgs "herdr-agent-parser" {};

    sessions = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = ["default"];
      description = "herdr-core sessions (HC_SESSION) to run a daemon for, one each.";
    };

    claudeHooks = lib.mkOption {
      type = lib.types.bool;
      default = config.programs.claude-code.enable;
      defaultText = lib.literalExpression "config.programs.claude-code.enable";
      description = ''
        Add the daemon's hooks to Claude Code's settings. Without them it
        falls back to finding each terminal's session through the process
        tree and ~/.claude/sessions/<pid>.json, which is undocumented, and
        loses live status and permission prompts.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    # On PATH for the CLI verbs (list, get, wait, ...) and for the phone's
    # `ssh ... herdr-agent-parser connect`.
    home.packages = [cfg.package];

    systemd.user.services = lib.listToAttrs (map (session:
      lib.nameValuePair (unitName session) {
        Unit.Description = "herdr-agent-parser for herdr-core session ${session}";
        Service = {
          ExecStart = "${exe} serve";
          Environment = ["HC_SESSION=${session}"];
          Restart = "on-failure";
          RestartSec = 5;
        };
        Install.WantedBy = ["default.target"];
      })
    cfg.sessions);

    programs.claude-code.settings.hooks = lib.mkIf cfg.claudeHooks (lib.genAttrs hookEvents (_: hookEntry));
  };
}
