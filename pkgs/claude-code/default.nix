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
  version = "1.0.30";

  src = fetchzip {
    url = "https://registry.npmjs.org/@anthropic-ai/claude-code/-/claude-code-${version}.tgz";
    hash = "sha256-DwzSXpDrNV8FhfqrRQ3OK/LjmiXd+VHEW91jnyds2P4=";
  };

  npmDepsHash = "sha256-M6H6A4i4JBqcFTG/ZkmxpINa4lw8sO5+iu2YcBqmvi4=";

  postPatch = ''
    cp ${./package-lock.json} package-lock.json
  '';

  # `claude-code` tries to auto-update by default, this disables that functionality.
  # Note that the `DISABLE_AUTOUPDATER` environment variable is not documented, so this trick may
  # not continue to work.
  postInstall = ''
    wrapProgram $out/bin/claude \
      --set DISABLE_AUTOUPDATER 1 \
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