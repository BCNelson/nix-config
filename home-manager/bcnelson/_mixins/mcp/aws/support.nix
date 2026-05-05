{pkgs, config, ...}: {
  programs.mcp.servers.aws-support-mcp-server = {
    command = "${pkgs.uv}/bin/uvx";
    args = [
      "awslabs.aws-support-mcp-server@latest"
      "--debug"
      "--log-file"
      "${config.home.homeDirectory}/.cache/mcp/logs/mcp_aws_support_server.log"
    ];
  };
}
