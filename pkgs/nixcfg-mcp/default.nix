{
  buildGoModule,
  lib,
}:
buildGoModule {
  pname = "nixcfg-mcp";
  version = "0.1.0";

  src = ./.;

  vendorHash = "sha256-MTIL8OBENH9xnrdRwHBe4D9PZ/nX17sUDdUUB7GR0qA=";

  # Unit tests only: the rest needs the network, hosts and a nix-config checkout.
  checkFlags = [ "-short" ];

  # One binary, two names: `nixcfg` for the CLI, `nixcfg-mcp` (serve by
  # default) for MCP configs.
  postInstall = ''
    ln -s $out/bin/nixcfg-mcp $out/bin/nixcfg
  '';

  meta = {
    description = "Agent tooling for nix-config: deploy-timeline waits, host/CI status, build jobs, secret checks, guard hook, MCP server";
    license = lib.licenses.mit;
    maintainers = [];
    mainProgram = "nixcfg-mcp";
  };
}
