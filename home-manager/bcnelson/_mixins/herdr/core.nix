{ config, lib, pkgs, ... }:

let
  # herdr has no declarative layout: `herdr --default-config` describes no panes
  # or tabs at all, and nothing in config.toml can. Layouts exist only as socket
  # API calls, so a "default workspace" has to be a script that makes them.
  #
  # Tabs rather than splits, so each tool gets the full terminal and herdr's
  # stock `switch_tab = "prefix+1..9"` lands on agent/lazygit/files/shell in
  # order.
  #
  # Where the workspace goes is asked, not guessed. The focused pane's cwd is a
  # poor default for this: you are normally already inside one project and
  # reaching for a different one, so it would be wrong most of the time. yazi
  # browses to the answer instead, and doubles as the "files" tab below.
  #
  # runtimeInputs covers what the script itself calls; herdr runs it via
  # `/bin/sh -c` from the server process, which inherits the terminal's
  # environment rather than a login shell's.
  #
  # The per-tab commands are deliberately NOT on that PATH. `pane run` types
  # into the pane's own shell, so those resolve against the user profile, not
  # this derivation - a bare `yazi` there hits fish's command-not-found handler
  # on any host that has not activated the generation adding it. Absolute store
  # paths sidestep the profile entirely, same as the alt+g popup below. That is
  # why yazi appears twice: once here as the picker this script runs, once as a
  # store path typed into a pane.
  workspaceLayout = pkgs.writeShellApplication {
    name = "herdr-workspace-layout";
    runtimeInputs = [ pkgs.herdr pkgs.jq pkgs.yazi ];
    text = ''
      # Popups get the env half of herdr's custom_command_env even though they
      # are not started in the pane's directory - the alt+g binding below relies
      # on the same thing. Only used to decide where browsing starts.
      start=''${HERDR_ACTIVE_PANE_CWD:-$HOME}

      cwd_file=$(mktemp)
      trap 'rm -f "$cwd_file"' EXIT

      # `q` writes the directory yazi is sitting in; `Q` (quit --no-cwd-file)
      # leaves the file empty, which is the cancel path - so a stray alt+n does
      # not strand an unwanted workspace.
      yazi --cwd-file "$cwd_file" "$start"

      cwd=$(cat "$cwd_file")
      if [ -z "$cwd" ] || [ ! -d "$cwd" ]; then
        exit 0
      fi

      ws=$(herdr workspace create --cwd "$cwd" --label "$(basename "$cwd")" --focus)
      wsid=$(jq -er '.result.workspace.workspace_id' <<<"$ws")

      # A new workspace already owns one tab, so it becomes the agent tab rather
      # than being created. It is left as a bare shell deliberately: which agent
      # to run is a per-workspace decision, not a config-time one.
      agent_tab=$(jq -er '.result.tab.tab_id' <<<"$ws")
      herdr tab rename "$agent_tab" agent >/dev/null

      # tab create reports the pane it opened; `pane run` types into that pane's
      # shell, so a tab with no command just stays an interactive shell.
      mktab() {
        local label=$1
        shift
        local created pane
        created=$(herdr tab create --workspace "$wsid" --cwd "$cwd" --label "$label" --no-focus)
        pane=$(jq -er '.result.root_pane.pane_id' <<<"$created")
        if [ "$#" -gt 0 ]; then
          herdr pane run "$pane" "$@" >/dev/null
        fi
      }

      mktab lazygit ${pkgs.lazygit}/bin/lazygit
      mktab files ${pkgs.yazi}/bin/yazi
      mktab shell

      # Created with --no-focus so the tabs land in order; hand the workspace
      # back on the agent tab.
      herdr tab focus "$agent_tab" >/dev/null
    '';
  };
in

# Terminal multiplexer for coding agents - https://herdr.dev
#
# This is the half of herdr every machine wants: the binary and the config.
# The agent integrations (claude/codex/opencode/pi hooks, and the packages they
# drag in) live in ./default.nix, which workstation.nix imports. A server is a
# `herdr --remote` target rather than somewhere agents get launched by hand, so
# it takes this file alone -- ../../default.nix pulls it in for every non-thin
# host.
#
# herdr replaced tmux here entirely; see nixos/common.nix for where tmux is
# still kept alive for the users and machines that did not move.

{
  imports = [ ../../../_mixins/services/config-merge.nix ];

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

      experimental = {
        pane_history = true;

        # Remote hosts are reached with `herdr --remote <target>`, which starts
        # a local client -- and that client is normally launched from inside an
        # existing herdr pane, which trips herdr's nested-launch guard. Without
        # this, every remote attach dies on "nested herdr is disabled by
        # default". The guard is only advisory: it keys off HERDR_ENV /
        # HERDR_PANE_ID, so it would not have caught a remote herdr reached over
        # plain ssh anyway.
        allow_nested = true;
      };

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
        {
          # Browse to a directory, then get a new workspace there with the
          # agent/lazygit/files/shell tabs. A popup rather than "shell" because
          # the picker is interactive and needs a terminal to draw in; it closes
          # itself once the script returns.
          key = "alt+n";
          type = "popup";
          command = lib.getExe workspaceLayout;
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
}
