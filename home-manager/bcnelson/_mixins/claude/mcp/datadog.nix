{pkgs, ...}: {
  programs.claude-code = {
    mcpServers = {
      datadog = {
        type = "http";
        url = "https://mcp.datadoghq.com/api/unstable/mcp-server/mcp";
      };
    };
  };
}
