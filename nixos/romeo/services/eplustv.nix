{ config, pkgs, ... }:
let
  dataDirs = config.data.dirs;

  domain = "eplustv.romeo.b.nel.family";
  hostPort = 8084;

  # Shared with jellyfin so its Live TV tuner can reach EPlusTV by name
  # (http://eplustv:8000/channels.m3u) without leaving podman. EPlusTV builds
  # the stream URLs inside the M3U from the host the playlist was requested on,
  # so going through this network keeps playback on it too: Jellyfin pulls the
  # streams from eplustv:8000 and remuxes them to its own clients.
  network = "eplustv";
  networkEnsure = [ "-${pkgs.podman}/bin/podman network create ${network}" ];
in
{
  virtualisation.oci-containers.containers.eplustv = {
    image = "docker.io/tonywagner/eplustv:latest";
    ports = [ "127.0.0.1:${toString hostPort}:8000" ];
    volumes = [
      # The DB plus every provider's login tokens, so level3 (backed up
      # offsite): losing it means re-linking each TV-provider account by hand.
      "${dataDirs.level3}/eplustv:/app/config"
    ];
    environment = {
      # The entrypoint creates a user with these ids and su-execs npm into it;
      # without them the app runs as root.
      PUID = "1000";
      PGID = "1000";
    };
    extraOptions = [ "--network=${network}" ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  # Keep jellyfin on podman's default network as well, so nothing about how it
  # is published or reaches the internet changes; the second network only adds
  # a route (and aardvark DNS) to eplustv.
  virtualisation.oci-containers.containers.jellyfin.extraOptions = [
    "--network=podman"
    "--network=${network}"
  ];

  systemd.services.podman-eplustv.serviceConfig.ExecStartPre = networkEnsure;
  systemd.services.podman-jellyfin.serviceConfig.ExecStartPre = networkEnsure;

  systemd.tmpfiles.rules = [
    "d ${dataDirs.level3}/eplustv 0700 1000 1000 - -"
  ];

  # The admin UI has no authentication of its own and holds live credentials
  # for every linked provider, so it is tailnet-only. *.romeo.b.nel.family is
  # a wildcard to romeo's Tailscale address (main.tf), and the h.b ingress is a
  # plain port forward that preserves the client address, so anything arriving
  # from outside Tailscale's ranges is refused here.
  services.nginx.virtualHosts."${domain}" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      allow 100.64.0.0/10;
      allow fd7a:115c:a1e0::/48;
      deny all;
    '';
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString hostPort}";
      proxyWebsockets = true;
    };
  };
}
