{ libx, dataDirs, pkgs, ... }:
let
  dns_linode_key = libx.getSecret ../../sensitive.nix "dns_linode_key";
  linodeToken = pkgs.writeTextFile {
    name = "linode-dns-config";
    text = ''
      dns_linode_key = ${dns_linode_key}
    '';
    destination = "/linode.ini";
  };
  swag = import ./defs/swag.nix { inherit dataDirs linodeToken; };
  jellyfin = import ./defs/jellyfin.nix { inherit dataDirs; };
  audiobooks = import ./defs/audiobooks.nix { inherit dataDirs; };
  nextcloud = import ./defs/nextcloud.nix { inherit dataDirs libx; };
  vikunja = import ./defs/vikunja.nix { inherit dataDirs libx; };
  mealie = import ./defs/mealie.nix { inherit dataDirs libx; };
  syncthing = import ./defs/syncthing.nix { inherit dataDirs; };
  foundryvtt = import ./defs/foundryvtt.nix { inherit dataDirs libx; };
  fastenhealth = import ./defs/fastenhealth.nix { inherit dataDirs; };
  homebox = import ./defs/homebox.nix { inherit dataDirs; };
  immich = import ./defs/immich.nix { inherit dataDirs libx; };
  dashy = import ./defs/dashy { inherit pkgs; };
  authelia = import ./defs/authelia { inherit dataDirs libx pkgs; };
  paperless = import ./defs/paperless { inherit dataDirs libx; };
in
{
  networkBacked = libx.createDockerComposeStackPackage {
    name = "general";
    src = ./defs/config;
    dependencies = [ linodeToken ];
    dockerComposeDefinition = {
      version = "3.8";
      services = builtins.foldl' (a: b: a // b) { } [
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
        immich
        dashy
        authelia
        paperless
      ];
    };
  };
}
