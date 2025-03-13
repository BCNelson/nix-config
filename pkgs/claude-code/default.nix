{
  lib,
  buildNpmPackage,
  fetchzip,
  bash,
  findutils,
  gnumake,
  gnused,
  gnugrep,
  coreutils-full,
  ripgrep
}:

buildNpmPackage rec {
  pname = "claude-code";
  version = "0.2.32";

  src = fetchzip {
    url = "https://registry.npmjs.org/@anthropic-ai/claude-code/-/claude-code-${version}.tgz";
    hash = "sha256-yIj/+m1LKIN51X5zmBnayucpAzz2cdE/QXfwQapeEqI=";
  };

  npmDepsHash = "sha256-NkMPBbLgr6MuMWNswDsulAYR7A8M6H9EGF2rw1tC33E=";

  postPatch = ''
    cp ${./package-lock.json} package-lock.json
  '';

  postFixup = ''
    wrapProgram $out/bin/claude \
      --prefix PATH ${lib.makeBinPath [
        coreutils-full
        findutils
        gnumake
        gnused
        gnugrep
        bash
        ripgrep
      ]}
  '';

  dontNpmBuild = true;

  AUTHORIZED = "1";

  passthru.updateScript = ./update.sh;

  meta = {
    description = "An agentic coding tool that lives in your terminal, understands your codebase, and helps you code faster";
    homepage = "https://github.com/anthropics/claude-code";
    downloadPage = "https://www.npmjs.com/package/@anthropic-ai/claude-code";
    license = lib.licenses.unfree;
    maintainers = [ lib.maintainers.malo ];
    mainProgram = "claude";
  };
}