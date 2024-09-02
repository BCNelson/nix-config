args@{ pkgs, libx, inputs, ... }:
let
  dataDirs = import ./dataDirs.nix;

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
      peerPort = peerPort;
      flood.enable = true;
      extraAllowedIps = [ "100.*.*.*" ];
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
  };
}