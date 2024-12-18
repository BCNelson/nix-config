{ config, pkgs, ... }:
let
  dataDirs = config.data.dirs;
in
{
  services.immich = {
    enable = true;
    mediaLocation = "${dataDirs.level2}/photos";
    settings.server.externalDomain = "https://photos.ck.nel.family";
  };

  # Hardware Accelerated Transcoding using VA-API
  hardware.graphics = {
    enable = true;
    extraPackages = with pkgs; [
      intel-media-driver # For Broadwell (2014) or newer processors. LIBVA_DRIVER_NAME=iHD
    ];
  };

  users.users.immich.extraGroups = [ "video" "render" ];

  services.nginx.virtualHosts."photos.ck.nel.family" = {
    enableACME = true;
    forceSSL = true;
    locations."/" = {
      proxyPass = "http://[::1]:${toString config.services.immich.port}";
      proxyWebsockets = true;
    };
  };
}
