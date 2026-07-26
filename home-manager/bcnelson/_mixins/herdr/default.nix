{ config, lib, pkgs, ... }:
let
  jsonFormat = pkgs.formats.json { };

  # `herdr integration install <agent>` fs::writes these hook scripts into each
  # agent's config dir and edits that agent's config in place. Both are
  # read-only /nix/store symlinks here, so declare the end state instead and
  # never run the installer. Sourcing the assets out of herdr.src keeps the
  # HERDR_INTEGRATION_VERSION marker locked to the herdr package, so
  # `herdr integration status` stays "current" across updates.
  hookAsset = agent: "${pkgs.herdr.src}/src/integration/assets/${agent}/herdr-agent-state.sh";

  claudeHook = "${config.programs.claude-code.configDir}/hooks/herdr-agent-state.sh";
  codexHook = "${config.xdg.configHome}/codex/herdr-agent-state.sh";

  # Mirrors herdr's hook_command(): `bash '<path>' session`
  hookCommand = path: "bash '${path}' session";
in
{
  imports = [ ../../../_mixins/services/config-merge.nix ];

  # Terminal multiplexer for coding agents - https://herdr.dev
  #
  # `settings` is deliberately left empty: the home-manager module would write
  # config.toml as a read-only symlink, but herdr's Settings tab rewrites that
  # same file. config-merge owns it instead (below).
  programs.herdr.enable = true;

  services.config-merge.herdr = {
    live = "${config.xdg.configHome}/herdr/config.toml";

    # herdr's server caches config.toml at startup, so rewriting the file on
    # disk is invisible to a running session. This is the same nudge the
    # home-manager module uses in its own onChange. It fails harmlessly when no
    # server is up.
    onChange = "${lib.getExe pkgs.herdr} server reload-config";

    # preserveUnknown stays on for herdr: its Settings tab writes a moving
    # target, and enumerating those keys means every upstream addition is
    # silently discarded until someone notices. Anything `settings` declares
    # still wins, so this only widens what survives - the trade-off is that
    # removing a key from `settings` later leaves herdr's last runtime value
    # frozen in place rather than reverting to its own default.
    preserveUnknown = true;

    # Lifted verbatim from ~/.config/herdr/config.toml after running the
    # onboarding wizard, so a fresh machine lands on the same setup instead of
    # prompting again. Anything herdr adds beyond this still persists via the
    # overlay; these keys are the ones Nix enforces.
    settings = {
      onboarding = false;

      experimental.pane_history = true;

      # An absolute path rather than a bare name: herdr's server is started by
      # a terminal, not a login shell, so PATH is not guaranteed. Same approach
      # as ../../../_mixins/programs/tmux.nix.
      terminal.default_shell = "${pkgs.fish}/bin/fish";

      # Nix owns this binary, so `herdr update` could only fail against a
      # read-only store path. Silence the background checks and take updates
      # through `nix flake update` instead.
      update = {
        version_check = false;
        manifest_check = false;
      };

      keys.command = [
        {
          # No "prefix+", so this is a direct chord - herdr grabs alt+g
          # globally rather than after the prefix.
          key = "alt+g";
          type = "popup";
          # herdr drops the focused pane's cwd for popups (it passes None and
          # keeps only the env half of custom_command_env), but it still
          # exports HERDR_ACTIVE_PANE_CWD, and popup commands run through
          # `/bin/sh -c` - so cd there ourselves.
          command = ''cd "''${HERDR_ACTIVE_PANE_CWD:-$HOME}" && exec ${pkgs.lazygit}/bin/lazygit'';
          width = "80%";
          height = "80%";
        }
      ];

      ui = {
        agent_panel_sort = "spaces";
        show_agent_labels_on_pane_borders = true;
        toast.delivery = "system";
      };
    };
  };

  # The hook silently exits 0 without python3 on PATH, taking session
  # resume-after-restart with it.
  home.packages = [
    pkgs.python3
    # Project-local .mcp.json launches these outside `nix develop` as well.
    pkgs.devenv
    pkgs.ssh-mcp
  ];

  home.file."${claudeHook}".source = hookAsset "claude";

  programs.claude-code.settings.hooks.SessionStart = [
    {
      matcher = "*";
      hooks = [
        {
          type = "command";
          command = hookCommand claudeHook;
          timeout = 10;
        }
      ];
    }
  ];

  # Codex keeps its hook next to the config rather than in a hooks/ subdir, and
  # its SessionStart entry carries no matcher key.
  xdg.configFile = {
    "codex/herdr-agent-state.sh".source = hookAsset "codex";

    "codex/hooks.json".source = jsonFormat.generate "codex-hooks.json" {
      hooks.SessionStart = [
        {
          hooks = [
            {
              type = "command";
              command = hookCommand codexHook;
              timeout = 10;
            }
          ];
        }
      ];
    };
  };
}
