{ config, inputs, lib, pkgs, ... }:
let
  jsonFormat = pkgs.formats.json { };

  # Pi loads TypeScript extensions directly. Keep each extension together with
  # its locked runtime dependencies so it never relies on `pi install` state.
  piMcpAdapterSrc = pkgs.runCommand "pi-mcp-adapter-source" { nativeBuildInputs = [ pkgs.jq ]; } ''
    cp -r ${inputs.pi-mcp-adapter}/. "$out"
    chmod -R u+w "$out"

    # Upstream's lockfile omits these nested dependency integrities. Nix's
    # fetcher requires them, so fill only those missing fields with the npm
    # registry's published values for the locked tarballs.
    jq '
      .packages["node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-agent-core"].integrity = "sha512-XKxgdjhcPuyjrthCOFSgfzT3xZ1uBrJ1IMVDxci1to6hIN6BIg9J5iY8q0pGXK1DLgATLP23da+1UyZLwA360Q=="
      | .packages["node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-ai"].integrity = "sha512-9jR23tOl0BIUdQMn70Gr72xYBpM7Xgl9Lyv7gAnU1USfkNRuYG/f/edLl+n/Dp/RafDW3JI4DF7y/GhgkORuew=="
      | .packages["node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-tui"].integrity = "sha512-FUVOjDn1DVwM1uHD5MNYboXQrXjIDbSt+BQ3py7nQWCY62tKfxgiM1OBMxTcwRWLfSdZHUPpV0hm1loIdUJnPw=="
    ' "$out/package-lock.json" > "$out/package-lock.json.tmp"
    mv "$out/package-lock.json.tmp" "$out/package-lock.json"
  '';

  piMcpAdapter = pkgs.buildNpmPackage {
    pname = "pi-mcp-adapter";
    version = "2.14.0";
    src = piMcpAdapterSrc;
    npmDepsHash = "sha256-ZcKqb1f2hMVuLU1AFu3ebS62p/+57dQd2/g3nX1+uo4=";
    npmDepsFetcherVersion = 2;
    dontNpmBuild = true;

    installPhase = ''
      mkdir -p "$out/node_modules"
      cp -r node_modules/. "$out/node_modules"
      cp -r . "$out/node_modules/pi-mcp-adapter"
    '';
  };

  piPermissionSystem = pkgs.stdenv.mkDerivation {
    pname = "pi-permission-system";
    version = "23.0.1";
    src = inputs.pi-permission-system;
    pnpmDeps = pkgs.fetchPnpmDeps {
      pname = "pi-permission-system";
      version = "23.0.1";
      src = inputs.pi-permission-system;
      hash = "sha256-EfhJBD9m64y7Zzjjnv4IiOp3U9tHNdJhbheN3+7q9hw=";
      fetcherVersion = 4;
      pnpm = pkgs.pnpm_10;
    };
    nativeBuildInputs = [ pkgs.pnpmConfigHook pkgs.pnpm_10 ];

    installPhase = ''
      # pnpm workspace links point to sibling packages, so keep the workspace
      # layout intact rather than copying only the extension package.
      cp -r . "$out"
    '';
  };

  piPermissionConfig = {
    permission = {
      "*" = "allow";
      edit = "ask";
      write = "ask";
      bash = {
        "*" = "ask";
        "rm -rf *" = "deny";
        "sudo *" = "deny";
      };
      mcp."*" = "ask";
      external_directory = "ask";
      path = {
        "*" = "allow";
        "*.env" = "deny";
        "*.env.*" = "deny";
        "*.env.example" = "allow";
        "~/.ssh/*" = "deny";
      };
    };
  };

  # `herdr integration install <agent>` fs::writes these hook/plugin assets into
  # each agent's config dir and edits that agent's config in place. Both are
  # read-only /nix/store symlinks here, so declare the end state instead and
  # never run the installer. Sourcing the assets out of herdr.src keeps the
  # HERDR_INTEGRATION_VERSION marker locked to the herdr package, so
  # `herdr integration status` stays "current" across updates.
  asset = agent: file: "${pkgs.herdr.src}/src/integration/assets/${agent}/${file}";
  hookAsset = agent: asset agent "herdr-agent-state.sh";

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
    pkgs.pi-coding-agent
    # Project-local .mcp.json launches these outside `nix develop` as well.
    pkgs.devenv
    pkgs.ssh-mcp
  ];

  home.file."${claudeHook}".source = hookAsset "claude";

  home.file = {
    # Pi resolves relative imports from the extension path, not a symlink's
    # target. Use wrappers so each package resolves its own sibling modules and
    # store-vendored dependencies from the absolute import location.
    ".pi/agent/extensions/pi-mcp-adapter.ts".text = ''
      export { default } from "${piMcpAdapter}/node_modules/pi-mcp-adapter/index.ts";
    '';
    ".pi/agent/extensions/pi-permission-system.ts".text = ''
      export { default } from "${piPermissionSystem}/packages/pi-permission-system/src/index.ts";
    '';
    ".pi/agent/extensions/pi-permission-system/config.json".source = jsonFormat.generate "pi-permission-system.json" piPermissionConfig;
  };

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

    # opencode has no hook mechanism: herdr ships a JS plugin that subscribes to
    # session events instead. opencode auto-loads every file under plugins/, so
    # unlike claude and codex there is no config edit to mirror - dropping the
    # file in place is the whole integration. It is self-contained (only
    # node:net) and runs inside opencode's own runtime, so it needs nothing on
    # PATH. See ../opencode.
    #
    # herdr looks for this under a hardcoded ~/.config/opencode rather than
    # $XDG_CONFIG_HOME, so `herdr integration status opencode` only agrees with
    # this path while xdg.configHome is the default.
    "opencode/plugins/herdr-agent-state.js".source = asset "opencode" "herdr-agent-state.js";

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

  # Pi auto-loads extensions from its agent directory. This extension reports
  # lifecycle state and session identity while Pi runs in a Herdr pane.
  home.file.".pi/agent/extensions/herdr-agent-state.ts".source = asset "pi" "herdr-agent-state.ts";
}
