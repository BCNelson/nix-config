{pkgs, ...}: {
  programs.claude-code = {
    mcpServers = {
      playwrite = {
        type = "stdio";
        command = "${pkgs.nodejs}/bin/npx";
        args = [ "@playwright/mcp:latest" ];
      };
    };
  };  
}