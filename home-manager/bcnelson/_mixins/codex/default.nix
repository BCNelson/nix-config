{ config, lib, pkgs, ... }:
let
  tomlFormat = pkgs.formats.toml { };
  codexConfigDir = "codex";

  transformedMcpServers = lib.optionalAttrs config.programs.mcp.enable (
    lib.mapAttrs (
      _name: server:
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
    ) config.programs.mcp.servers
  );

  baseSettings =
    {
      experimental_use_rmcp_client = true;

      features = {
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
  home.sessionVariables = {
    CODEX_HOME = codexHome;
  };

  programs.codex = {
    enable = true;
  };

  xdg.configFile."${codexConfigDir}/config.base.toml".source = tomlFormat.generate "codex-config-base" baseSettings;

  systemd.user.services.codex-config-merge = {
    Unit = {
      Description = "Codex config merge daemon";
    };

    Service = {
      ExecStart = "${pkgs.codex-config-merge}/bin/codex-config-merge --base ${codexHome}/config.base.toml --runtime ${codexHome}/config.runtime.toml --live ${codexHome}/config.toml";
      Restart = "on-failure";
      RestartSec = 2;
    };

    Install = {
      WantedBy = [ "default.target" ];
    };
  };
}
