{ config, lib, pkgs, ... }:
let
  cfg = config.services.mcpproxy;
  json = pkgs.formats.json { };
  base = json.generate "mcpproxy-base.json" (cfg.settings // {
    listen = "127.0.0.1:${toString cfg.port}";
    data_dir = cfg.stateDirectory;
    require_mcp_auth = false;
    profiles = lib.mapAttrsToList (name: servers: { inherit name servers; }) cfg.profiles;
    mcpServers = lib.mapAttrsToList (name: server: {
      inherit name;
      protocol = "http";
      enabled = true;
      # These upstreams are explicitly trusted by the Nix configuration.
      quarantined = false;
    } // server) cfg.upstreams;
  });
  prepare = pkgs.writeShellScript "mcpproxy-prepare" ''
    set -eu
    umask 077
    state=${lib.escapeShellArg cfg.stateDirectory}
    ${pkgs.coreutils}/bin/mkdir -p "$state"
    ${pkgs.coreutils}/bin/chmod 700 "$state"
    live="$state/mcp_config.json"
    previous="$live"
    if [ ! -f "$previous" ]; then previous=${pkgs.writeText "mcpproxy-empty.json" "{}"}; fi
    temporary="$(${pkgs.coreutils}/bin/mktemp "$state/config.XXXXXX")"
    trap '${pkgs.coreutils}/bin/rm -f "$temporary"' EXIT
    # Nix owns policy. Preserve only the generated admin key and OAuth
    # registration fields for upstreams whose name and URL have not changed.
    ${pkgs.jq}/bin/jq -s '
      .[0] as $old | .[1] |
      .api_key = ($old.api_key // "") |
      .mcpServers |= map(. as $new |
        ([$old.mcpServers[]? | select(.name == $new.name and .url == $new.url)][0] // {}) as $previous |
        if $previous.oauth then .oauth = ($previous.oauth * (.oauth // {})) else . end)
    ' "$previous" ${base} > "$temporary"
    ${pkgs.coreutils}/bin/mv "$temporary" "$live"
  '';
  cli = pkgs.writeShellScriptBin "mcpproxy" ''
    exec ${lib.getExe cfg.package} --config ${lib.escapeShellArg "${cfg.stateDirectory}/mcp_config.json"} \
      --data-dir ${lib.escapeShellArg cfg.stateDirectory} "$@"
  '';
  # The web UI authenticates with the generated admin key as an ?apikey= query
  # parameter, and `status --web-url` prints the URL with it filled in. Reading
  # it at launch rather than baking it in keeps the key out of the Nix store and
  # follows `status --reset-key`. The key does land in browser history, which is
  # no wider than the 0700 state directory it already sits in for this user.
  dashboard = pkgs.writeShellScriptBin "mcpproxy-dashboard" ''
    if ! url="$(${lib.getExe cli} status --web-url)" || [ -z "$url" ]; then
      ${pkgs.libnotify}/bin/notify-send -a MCPProxy -i dialog-error \
        "MCPProxy dashboard" "Could not read the web UI URL; is the mcpproxy user service running?"
      exit 1
    fi
    exec ${pkgs.xdg-utils}/bin/xdg-open "$url"
  '';

in
{
  options.services.mcpproxy = {
    enable = lib.mkEnableOption "the shared local MCP authentication gateway";
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.mcpproxy;
      description = "MCPProxy package including its web UI.";
    };
    port = lib.mkOption { type = lib.types.port; default = 8640; description = "Loopback HTTP port."; };
    stateDirectory = lib.mkOption {
      type = lib.types.str;
      default = "${config.xdg.stateHome}/mcpproxy";
      description = "Private writable directory containing OAuth state and gateway credentials.";
    };
    upstreams = lib.mkOption {
      type = lib.types.attrsOf json.type;
      default = { };
      description = "Upstream MCPProxy server definitions keyed by stable name. Never embed secrets here.";
    };
    profiles = lib.mkOption {
      type = lib.types.attrsOf (lib.types.listOf lib.types.str);
      default = { };
      description = "Named server subsets at /mcp/p/<name>. These are convenience filters, not access controls.";
    };
    settings = lib.mkOption {
      inherit (json) type;
      default = { };
      description = "Additional JSON settings; listener, authentication, profiles and upstreams are module-owned.";
    };

  };

  config = lib.mkIf cfg.enable {
    assertions = lib.mapAttrsToList (name: servers: {
      assertion = builtins.match "[a-z0-9][a-z0-9_-]{0,62}" name != null
        && !(builtins.elem name [ "all" "code" "call" "p" ])
        && builtins.all (s: builtins.hasAttr s cfg.upstreams) servers;
      message = "services.mcpproxy.profiles.${name}: use a valid profile slug and known upstream names.";
    }) cfg.profiles;

    services.mcpproxy.settings = {
      routing_mode = lib.mkDefault "retrieve_tools";
      features.enable_web_ui = lib.mkDefault true;
      enable_code_execution = lib.mkDefault false;
      telemetry.enabled = lib.mkDefault false;
      update_check.enabled = false;
      activity_retention_days = lib.mkDefault 7;
    };

    home.packages = [ cli ] ++ lib.optional cfg.settings.features.enable_web_ui dashboard;

    # Indexed by KRunner and the application launcher.
    xdg.desktopEntries = lib.mkIf cfg.settings.features.enable_web_ui {
      mcpproxy-dashboard = {
        name = "MCPProxy Dashboard";
        genericName = "MCP Gateway";
        comment = "Open the MCPProxy web UI, signed in with the local admin key";
        exec = lib.getExe dashboard;
        icon = "network-server";
        terminal = false;
        categories = [ "Development" "Network" ];
        settings.Keywords = "mcp;mcpproxy;gateway;oauth;";
      };
    };

    systemd.user.services.mcpproxy = {
      Unit = {
        Description = "Shared MCP gateway";
        After = [ "network-online.target" ];
        Wants = [ "network-online.target" ];
      };
      Service = {
        ExecStartPre = "${prepare}";
        ExecStart = lib.escapeShellArgs [ (lib.getExe cfg.package) "serve"
          "--config" "${cfg.stateDirectory}/mcp_config.json" "--data-dir" cfg.stateDirectory ];
        # xdg-open delegates to desktop helpers (e.g. KDE's kde-open), and
        # browser desktop entries may resolve executables from the user profile.
        # /usr/bin is where those helpers live on non-NixOS hosts (redo-3).
        Environment = [ "PATH=${lib.makeBinPath [ pkgs.coreutils pkgs.xdg-utils ]}:${config.home.profileDirectory}/bin:/run/current-system/sw/bin:/usr/bin" ];
        UMask = "0077";
        Restart = "on-failure";
        RestartSec = 5;
        TimeoutStartSec = 180;
      };
      Install.WantedBy = [ "default.target" ];
    };
  };
}
