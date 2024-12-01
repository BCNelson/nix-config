{ libx, dataDirs, pkgs, ... }:
let
  jellyfin = import ./defs/jellyfin.nix { inherit dataDirs; };
  nextcloud = import ./defs/nextcloud.nix { inherit dataDirs libx; };
  vikunja = import ./defs/vikunja.nix { inherit dataDirs libx pkgs; };
  mealie = import ./defs/mealie.nix { inherit dataDirs libx; };
  syncthing = import ./defs/syncthing.nix { inherit dataDirs; };
  foundryvtt = import ./defs/foundryvtt.nix { inherit dataDirs libx; };
  fastenhealth = import ./defs/fastenhealth.nix { inherit dataDirs; };
  homebox = import ./defs/homebox.nix { inherit dataDirs; };
  immich = import ./defs/immich.nix { inherit dataDirs libx; };
  paperless = import ./defs/paperless.nix { inherit dataDirs libx; };
  tubearchivist = import ./defs/tubearchivist.nix { inherit dataDirs libx; };
  actual = import ./defs/actual.nix { inherit dataDirs; };
in
{
  networkBacked = libx.createDockerComposeStackPackage {
    name = "general";
    src = ./defs/config;
    dockerComposeDefinition = {
      services = builtins.foldl' (a: b: a // b) { } [
        jellyfin
        nextcloud
        vikunja
        mealie
        syncthing
        foundryvtt
        fastenhealth
        homebox
        immich
        paperless
        tubearchivist
        actual
      ];
    };
  };
}
