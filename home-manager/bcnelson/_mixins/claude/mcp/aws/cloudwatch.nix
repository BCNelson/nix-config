{pkgs, ...}: {
  programs.claude-code = {
    mcpServers = {
      aws-cloudwatch-mcp-server = {
        type = "stdio";
        command = "${pkgs.uv}/bin/uvx";
        args = [
          "awslabs.cloudwatch-mcp-server@latest"
        ];
        env = {
          FASTMCP_LOG_LEVEL = "ERROR";
        };
      };
    };
  };
}
