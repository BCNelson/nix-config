{ config, lib, ... }:
let
  dataDirs = config.data.dirs;
in
{
  services.bcnelson.binary-cache-proxy = {
      enable = true;
      domain = "nixcache.nel.family";
      cachePath = "${dataDirs.level7}/nixBinaryCacheProxy";
    };

  nix.settings.substituters = lib.mkBefore [ "https://nixcache.nel.family/" ];
}
