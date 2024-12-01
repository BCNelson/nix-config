{ config, pkgs, libx, inputs, ... }:
let
  dataDirs = config.data.dirs;

  wgConfig = libx.getSecret ./sensitive.nix "airdnsWGConfig";
  wgConfigText = pkgs.writeTextDir "wg.conf" wgConfig;
  peerPort = libx.getSecretWithDefault ./sensitive.nix "airdnsPeerPort" 0;
in
{
  imports =
    [
      inputs.nixarr.nixosModules.default
    ];

  nixarr = {
    enable = true;

    vpn = {
      enable = true;
      wgConf = "${wgConfigText}/wg.conf";
    };

    stateDir = "${dataDirs.level4}/nixarr";
    mediaDir = "${dataDirs.level6}/media";

    transmission = {
      enable = true;
      vpn.enable = true;
      inherit peerPort;
      flood.enable = true;
      extraAllowedIps = [ "100.*.*.*" ];
      extraSettings = {
        "rpc-host-whitelist" = "romeo.b.nel.family";
        "rpc-host-whitelist-enabled" = true;
      };
    };

    bazarr.enable = true;
    prowlarr.enable = true;
    radarr.enable = true;
    readarr.enable = true;
    sonarr.enable = true;
  };

  services.jackett = {
    enable = true;
    dataDir = "${dataDirs.level6}/jackett";
    openFirewall = true;
  };
}
