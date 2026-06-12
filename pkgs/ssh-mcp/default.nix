{
  buildGoModule,
  lib,
}:
buildGoModule {
  pname = "ssh-mcp";
  version = "0.1.0";

  src = ./.;

  vendorHash = "sha256-Mjx0lYFmgvNwbPLYiBPr5mGRij88Y30sddZtAG2GHwQ=";

  meta = {
    description = "MCP server that runs commands over SSH on an allow-list of hosts (keyless Tailscale SSH)";
    license = lib.licenses.mit;
    maintainers = [];
    mainProgram = "ssh-mcp";
  };
}
