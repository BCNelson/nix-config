{ config, lib, pkgs, ... }:
let
  codexConfigDir = "codex";

  # Same transform programs.codex.enableMcpIntegration applies upstream, done by
  # hand because config-merge - not the module - owns config.toml here. Beyond
  # renaming headers -> http_headers and wrapping file-backed env vars, the
  # shared helper drops null/empty attrs, which matters twice over: the TOML
  # formatter cannot serialize null ("unsupported unit type"), and codex's rmcp
  # client rejects even an empty `args` on a streamable_http server
  # ("args is not supported for streamable_http").
  transformedMcpServers = lib.optionalAttrs config.programs.mcp.enable (
    lib.mapAttrs (
      name: server:
      lib.hm.mcp.transformMcpServer {
        inherit server;
        exclude = [
          "headers"
          "type"
        ];
        extraTransforms = [
          (s: s // lib.optionalAttrs (s.headers or { } != { }) { http_headers = s.headers; })
          (lib.hm.mcp.wrapEnvFilesCommand { inherit pkgs name; })
        ];
      }
    ) config.programs.mcp.servers
  );

  baseSettings =
    {
      experimental_use_rmcp_client = true;

      features = {
        # Required by the herdr codex integration (see ../herdr)
        hooks = true;
        memories = true;
        prevent_idle_sleep = true;
        terminal_resize_reflow = true;
      };
    }
    // lib.optionalAttrs (transformedMcpServers != { }) {
      mcp_servers = transformedMcpServers;
    };

  codexHome = "${config.xdg.configHome}/${codexConfigDir}";
in
{
  imports = [ ../../../_mixins/services/config-merge.nix ];

  home.sessionVariables = {
    CODEX_HOME = codexHome;
  };

  programs.codex = {
    enable = true;
  };

  # Codex records per-project trust decisions and hook review state in the same
  # config.toml it reads, so the declarative half cannot be a read-only symlink.
  # config-merge owns the live file and replays that runtime state onto the base.
  services.config-merge.codex = {
    settings = baseSettings;
    live = "${codexHome}/config.toml";
    runtimeKeys = [
      "projects.*.trust_level"
      # Codex stores the review state for hooks from hooks.json here. Preserve
      # the whole table so trusting a hook survives the next reconciliation.
      "hooks"
    ];
  };
}
