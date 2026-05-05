{pkgs, ...}: {
  programs.mcp.servers.playwright = {
    command = "${pkgs.nodejs}/bin/npx";
    args = [ "-y" "@playwright/mcp@latest" ];
  };
}
