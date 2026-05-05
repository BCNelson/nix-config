{pkgs, ...}: {
  programs.mcp.servers.playwrite = {
    command = "${pkgs.nodejs}/bin/npx";
    args = [ "@playwright/mcp:latest" ];
  };
}
