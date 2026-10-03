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

  # A bare `herdr` starts the server if none is running -- as a child of the
  # client, so it lands in whichever Ghostty window's scope ran it first, along
  # with every pane and agent after it. That scope is one systemd-oomd watches,
  # so a single runaway build in any pane could take the whole server down.
  #
  # Start it ourselves first, in a scope of its own: weighted above ordinary
  # apps (agents stay responsive while their commands sit in lowprio.slice)
  # and outside oomd's reach. A scope launched from here rather than a user
  # service, so panes still inherit the terminal's environment -- PATH,
  # SSH_AUTH_SOCK and friends -- exactly as before.
  #
  # Only the plain local attach (optionally --session NAME) is intercepted;
  # every other subcommand, and anything that fails here, falls through to the
  # client, which then starts the server the old way.
  herdrLauncher = pkgs.writeShellApplication {
    name = "herdr";
    # Absolute paths, not runtimeInputs: the server inherits this script's
    # environment, and a PATH with extra store paths prepended would leak into
    # every pane.
    text = ''
      real=${lib.getExe pkgs.herdr}
      session=()
      unit=herdr-server

      if [ $# -eq 2 ] && [ "$1" = --session ]; then
        session=(--session "$2")
        unit="herdr-server-$(${pkgs.systemd}/bin/systemd-escape "$2")"
      elif [ $# -ne 0 ]; then
        exec "$real" "$@"
      fi

      running() {
        "$real" "''${session[@]}" status server 2>/dev/null | ${pkgs.gnugrep}/bin/grep -q 'status: running'
      }

      if [ -z "''${HERDR_ENV:-}" ] && ! running \
        && ${pkgs.systemd}/bin/busctl --user --quiet --timeout=2 call org.freedesktop.systemd1 /org/freedesktop/systemd1 \
          org.freedesktop.DBus.Peer Ping >/dev/null 2>&1; then
        ${pkgs.util-linux}/bin/setsid -f ${pkgs.systemd}/bin/systemd-run --user --scope --quiet --collect --unit="$unit" --slice=app.slice \
          -p CPUWeight=200 -p IOWeight=200 -p MemoryLow=4G \
          -- "$real" "''${session[@]}" server </dev/null >/dev/null 2>&1
        for _ in $(${pkgs.coreutils}/bin/seq 50); do
          running && break
          ${pkgs.coreutils}/bin/sleep 0.1
        done
      fi

      exec "$real" "$@"
    '';
  };
in
{
  # The binary and config.toml come from ./core.nix, which every non-thin host
  # takes. What is left in this file is the agent-integration layer, which only
  # makes sense where agents are actually launched by hand.
  imports = [ ./core.nix ./mirror.nix ];

  # The hook silently exits 0 without python3 on PATH, taking session
  # resume-after-restart with it.
  home.packages = [
    # Shadows the plain herdr that core.nix and the system profile provide.
    (lib.hiPrio herdrLauncher)
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

  # Pi writes its own state (last model, changelog version) back into
  # settings.json, so it cannot be a read-only symlink. shellPath is the
  # lowprio-routing bash from pkgs/lowprio; pi runs tool calls as
  # `<shellPath> -c <command>`. Its shellCommandPrefix would not do: that is
  # text prepended to the script, not a wrapper around the process.
  services.config-merge.pi = {
    settings.shellPath = "${pkgs.lowprio.agentShell}/bin/bash";
    live = "${config.home.homeDirectory}/.pi/agent/settings.json";
    preserveUnknown = true;
  };
}
