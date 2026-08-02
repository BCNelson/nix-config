{
  buildGoModule,
  fetchFromGitHub,
  lib,
}:
let
  # Upstream cuts releases roughly daily; pin deliberately and bump on purpose.
  # A bump almost always invalidates vendorHash as well.
  version = "7.2.113";
in
buildGoModule {
  pname = "cli-proxy-api";
  inherit version;

  src = fetchFromGitHub {
    owner = "router-for-me";
    repo = "CLIProxyAPI";
    tag = "v${version}";
    hash = "sha256-aget6PRnWkzNy/QAG54qCRjHfTRui3srplM+U73Hlbc=";
  };

  vendorHash = "sha256-CrDp7MOr+AwJUhTovklXx3F1yaktQlvD7VYhYSY6VvY=";

  # The repo also ships model-catalog helper commands (fetch_codex_models etc.)
  # that are only useful for upstream development; build just the server.
  subPackages = [ "cmd/server" ];

  ldflags = [
    "-s"
    "-w"
    "-X"
    "main.Version=${version}"
  ];

  # Upstream's tests reach the live Codex/Gemini endpoints, so they cannot run
  # in the sandbox.
  doCheck = false;

  # subPackages names the binary after its directory ("server").
  postInstall = ''
    mv $out/bin/server $out/bin/cli-proxy-api
  '';

  meta = {
    description = "OpenAI/Gemini/Claude-compatible API proxy backed by CLI subscription credentials";
    homepage = "https://github.com/router-for-me/CLIProxyAPI";
    license = lib.licenses.mit;
    maintainers = [];
    mainProgram = "cli-proxy-api";
    platforms = lib.platforms.linux;
  };
}
