{pkgs, ...}: {
  programs.claude-code = {
    mcpServers = {
      pulumi = {
        type = "http";
        url = "https://mcp.ai.pulumi.com/mcp";
      };
    };
  };
}
