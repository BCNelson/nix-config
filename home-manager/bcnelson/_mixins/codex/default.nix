{ config, lib, pkgs, ... }:
let
  tomlFormat = pkgs.formats.toml { };
  codexConfigDir = "codex";

  transformedMcpServers = lib.optionalAttrs config.programs.mcp.enable (
    lib.mapAttrs (
      _name: server:
      # Drop null-valued attrs: the home-manager mcp module declares
      # command/url/enabled as nullOr options defaulting to null, and the
      # TOML formatter cannot serialize null ("unsupported unit type").
      lib.filterAttrs (_: v: v != null) (
        (lib.removeAttrs server [
          "disabled"
          "headers"
        ])
        // (lib.optionalAttrs (server ? headers && !(server ? http_headers)) {
          http_headers = server.headers;
        })
        // {
          enabled = !(server.disabled or false);
        }
      )
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

  # Codex records per-project trust decisions in the same config.toml it reads,
  # so the declarative half cannot be a read-only symlink. config-merge owns the
  # live file and replays those decisions on top of the base.
  services.config-merge.codex = {
    settings = baseSettings;
    live = "${codexHome}/config.toml";
    runtimeKeys = [ "projects.*.trust_level" ];
  };
}
