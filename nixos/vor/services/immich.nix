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

  systemd.tmpfiles.settings."10-immich" = {
    "${config.services.immich.mediaLocation}" = {
      d = {
        user = "${config.services.immich.user}";
        group = "${config.services.immich.group}";
        mode = "0755";
      };
    };
  };

  environment.systemPackages = [
    pkgs.icloudpd
    pkgs.immich-cli
  ];

  services.nginx.virtualHosts."photos.ck.nel.family" = {
    enableACME = true;
    acmeRoot = null;
    forceSSL = true;
    locations."/" = {
      proxyPass = "http://[::1]:${toString config.services.immich.port}";
      proxyWebsockets = true;
    };
  };
}
