{pkgs, ...}: {
  programs.claude-code = {
    mcpServers = {
      kubernetes = {
        type = "stdio";
        command = "${pkgs.nodejs}/bin/npx";
        args = [ "-y" "kubernetes-mcp-server@latest" ];
      };
    };
  };
}
