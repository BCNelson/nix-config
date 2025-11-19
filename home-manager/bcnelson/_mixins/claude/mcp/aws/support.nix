{pkgs, ...}: {
  programs.claude-code = {
    mcpServers = {
      aws-support-mcp-server = {
        type = "stdio";
        command = "${pkgs.uv}/bin/uvx";
        args = [
          "awslabs.aws-support-mcp-server@latest"
          "--debug"
          "--log-file"
          "./logs/mcp_support_server.log"
        ];
      };
    };
  };
}