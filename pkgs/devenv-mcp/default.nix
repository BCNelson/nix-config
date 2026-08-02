{
  buildGoModule,
  lib,
}:
buildGoModule {
  pname = "devenv-mcp";
  version = "0.1.0";

  src = ./.;

  vendorHash = "sha256-hHW8MbgORLiGEEvjtp4n4SpGsFuplZ+gCJ1atGorlnA=";

  meta = {
    description = "MCP server that manages Nix devenv containers for AI agents via rootless Podman";
    license = lib.licenses.mit;
    maintainers = [];
    mainProgram = "devenv-mcp";
  };
}
