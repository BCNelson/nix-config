{ lib, stdenvNoCC, fetchurl }:
let
  version = "0.65.0";
  releases = {
    x86_64-linux = {
      arch = "amd64";
      hash = "sha256-OCvoMOKhZhP9upTPl0UBy1G6HJ5zvv/0tCW9iOJMjbk=";
    };
    aarch64-linux = {
      arch = "arm64";
      hash = "sha256-ExJ83m0Qnqz3Yid+MynO65uSj/CGPeavioKAi193FUk=";
    };
  };
  release = releases.${stdenvNoCC.hostPlatform.system};
in
stdenvNoCC.mkDerivation {
  pname = "mcpproxy";
  inherit version;
  # The static upstream release includes the compiled web UI.
  src = fetchurl {
    url = "https://github.com/smart-mcp-proxy/mcpproxy-go/releases/download/v${version}/mcpproxy-${version}-linux-${release.arch}.tar.gz";
    inherit (release) hash;
  };
  sourceRoot = ".";
  dontConfigure = true;
  dontBuild = true;
  installPhase = ''
    runHook preInstall
    install -Dm755 mcpproxy "$out/bin/mcpproxy"
    runHook postInstall
  '';
  doInstallCheck = true;
  installCheckPhase = ''
    $out/bin/mcpproxy --version | grep -F 'v${version}'
  '';
  meta = {
    description = "MCP gateway with shared OAuth, agent tokens, profiles and a web UI";
    homepage = "https://github.com/smart-mcp-proxy/mcpproxy-go";
    license = lib.licenses.mit;
    mainProgram = "mcpproxy";
    platforms = builtins.attrNames releases;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
