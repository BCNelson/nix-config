{config, pkgs, ...}: let
  kubernetesMcpPort = 8931;
in {
  programs.mcp.servers.kubernetes = {
    url = "http://127.0.0.1:${toString kubernetesMcpPort}/mcp";
    startup_timeout_sec = 20;
  };

  systemd.user.services.kubernetes-mcp-server = {
    Unit = {
      Description = "Kubernetes MCP Server";
      After = [ "network-online.target" ];
      Wants = [ "network-online.target" ];
    };

    Service = {
      ExecStart = "${pkgs.nodejs}/bin/npx -y kubernetes-mcp-server@latest --port ${toString kubernetesMcpPort}";
      Environment = [
        "KUBECONFIG=${config.home.homeDirectory}/.config/kube/config"
      ];
      Restart = "on-failure";
      RestartSec = 5;
    };

    Install = {
      WantedBy = [ "default.target" ];
    };
  };
}
