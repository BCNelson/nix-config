{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  services.bcnelson.binary-cache-proxy = {
      enable = true;
      domain = "nixcache.nel.family";
      cachePath = "${dataDirs.level7}/nixBinaryCacheProxy";
    };

  nix.binaryCaches = [ "https://nixcache.nel.family/" "http://cache.nixos.org/" ];
}
