{ config, lib, pkgs, ... }:
let
  kubernetesMcpPort = 8931;
in
{
  options.programs.mcp.kubernetes.enable = lib.mkEnableOption "the Kubernetes MCP server" // {
    default = true;
  };

  config = lib.mkIf config.programs.mcp.kubernetes.enable {
    programs.mcp.servers.kubernetes = {
      url = "http://127.0.0.1:${toString kubernetesMcpPort}/mcp";
      startup_timeout_sec = 20;
    };

    systemd.user.services.kubernetes-mcp-server = {
      Unit = {
        Description = "Kubernetes MCP Server";
        After = [ "network-online.target" ];
        Wants = [ "network-online.target" ];

        # Without a kubeconfig the server exits with "no current-context is set",
        # which `Restart = on-failure` then retries forever - this unit had logged
        # 100k+ restarts on hosts that never had one. Skip cleanly instead; it
        # starts on the next login once the file exists.
        ConditionPathExists = "${config.home.homeDirectory}/.config/kube/config";
      };

      Service = {
        ExecStart = "${pkgs.nodejs}/bin/npx -y kubernetes-mcp-server@latest --port ${toString kubernetesMcpPort}";
        Environment = [
          "KUBECONFIG=${config.home.homeDirectory}/.config/kube/config"

          # npx resolves on its own (absolute store shebang), but the binary it
          # execs carries `#!/usr/bin/env node`, and a systemd user unit's PATH
          # has no nodejs - that was the exit 127 in the crash loop.
          "PATH=${pkgs.nodejs}/bin"
        ];
        Restart = "on-failure";
        RestartSec = 5;
      };

      Install = {
        WantedBy = [ "default.target" ];
      };
    };
  };
}
