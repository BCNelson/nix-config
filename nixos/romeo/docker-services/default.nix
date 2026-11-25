{ libx, dataDirs, paperlessEnvFile, ... }:
let
  nextcloud = import ./defs/nextcloud.nix { inherit dataDirs libx; };
  paperless = import ./defs/paperless.nix {
    inherit dataDirs libx;
    envFile = paperlessEnvFile;
  };
  tubearchivist = import ./defs/tubearchivist.nix { inherit dataDirs libx; };
in
{
  networkBacked = libx.createDockerComposeStackPackage {
    name = "general";
    src = ./defs/config;
    dockerComposeDefinition = {
      services = builtins.foldl' (a: b: a // b) { } [
        nextcloud
        paperless
        tubearchivist
      ];
    };
  };
}
