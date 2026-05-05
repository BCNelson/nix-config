{pkgs, ...}: {
  programs.mcp.servers.aws-cloudwatch-mcp-server = {
    command = "${pkgs.uv}/bin/uvx";
    args = [
      "awslabs.cloudwatch-mcp-server@latest"
    ];
    env = {
      FASTMCP_LOG_LEVEL = "ERROR";
    };
  };
}
