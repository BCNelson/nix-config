{ libx, dataDirs, pkgs, ... }:
let
  sensitiveData = import ../../sensitive.nix;
  linodeToken = pkgs.writeTextFile {
    name = "linode-dns-config";
    text = ''
      dns_linode_key = ${sensitiveData.dns_linode_key}
    '';
    destination = "/linode.ini";
  };
  swag = import ./defs/swag.nix { inherit dataDirs linodeToken; };
  jellyfin = import ./defs/jellyfin.nix { inherit dataDirs; };
  audiobooks = import ./defs/audiobooks.nix { inherit dataDirs; };
  nextcloud = import ./defs/nextcloud.nix { inherit dataDirs; };
  vikunja = import ./defs/vikunja.nix { inherit dataDirs; };
  mealie = import ./defs/mealie.nix { inherit dataDirs; };
  syncthing = import ./defs/syncthing.nix { inherit dataDirs; };
  foundryvtt = import ./defs/foundryvtt.nix { inherit dataDirs; };
  fastenhealth = import ./defs/fastenhealth.nix { inherit dataDirs; };
  homebox = import ./defs/homebox.nix { inherit dataDirs; };
in
{
  networkBacked = libx.createDockerComposeStackPackage {
    name = "general";
    src = ./defs/config;
    dependencies = [ linodeToken ];
    dockerComposeDefinition = {
      version = "3.8";
      services = buildins.foldl' (a: b: a // b) { } [
        swag
        jellyfin
        audiobooks
        nextcloud
        vikunja
        mealie
        syncthing
        foundryvtt
        fastenhealth
        homebox
      ];
    };
  };
}
