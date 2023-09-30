{
  pkgs,
  stdenvNoCC,
}:
{
    name,
    src,
    dockerComposeDefinition,
}:
let
    startScript = ''
    #!/usr/bin/env bash
    set -euo pipefail
    pushd %outDir%
    docker-compose "$@"
    '';
in stdenv.mkDerivation {
    name = "${name}-docker-stack";
    src = src;
    buildInputs = [ pkgs.docker-compose pkgs.docker];
    installPhase = ''
    cp -r $src/* $out
    mkdir -p $out/bin
    echo "${startScript}" | sed "s+%outDir%+$out+" > $out/bin/dockerStack-${name}
    echo "${builtins.toJSON dockerComposeDefinition}" > $out/docker-compose.yml
    '';
};