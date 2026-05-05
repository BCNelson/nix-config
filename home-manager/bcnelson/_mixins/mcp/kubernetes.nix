{pkgs, ...}: {
  programs.mcp.servers.kubernetes = {
    command = "${pkgs.nodejs}/bin/npx";
    args = [ "-y" "kubernetes-mcp-server@latest" ];
  };
}
