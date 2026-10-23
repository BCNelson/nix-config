{
  config,
  lib,
  pkgs,
  ...
}:
let
  image = "docker://ghcr.io/catthehacker/ubuntu:act-22.04";

  # The runner and every job container run under this user's rootless Podman,
  # so job containers are confined to its user namespace instead of being
  # root on romeo. A fixed uid keeps the user socket path stable.
  runnerUser = "forgejo-runner";
  runnerUid = 1010;
  userManager = "user@${toString runnerUid}.service";
  podmanSocket = "/run/user/${toString runnerUid}/podman/podman.sock";

  # Rootless containers reach the host only through pasta's
  # host.containers.internal mapping: neither the job network's gateway nor
  # romeo's LAN IP answers from inside. Connections arrive as local traffic,
  # so wildcard-bound host ports are reachable without a firewall rule while
  # loopback-only services are not.
  hostFromJobs = "host.containers.internal";
  cacheProxyPort = 4101;
in
{
  age.secrets.forgejo_runner_token.rekeyFile = ./secrets/forgejo_runner_token.age;

  users.users.${runnerUser} = {
    # isNormalUser for the automatic subuid/subgid range rootless Podman needs.
    isNormalUser = true;
    uid = runnerUid;
    group = runnerUser;
    description = "Forgejo Actions runner (rootless Podman)";
    # Also holds the runner's state directory and Podman's image storage.
    home = "/var/lib/forgejo-runner";
    createHome = true;
    shell = "${pkgs.shadow}/bin/nologin";
    # Starts the user manager at boot, which brings up the podman.socket user
    # unit the Podman module enables for every user.
    linger = true;
  };
  users.groups.${runnerUser} = { };

  services.forgejo-runner.instances.romeo = {
    enable = true;

    # The module only knows rootful runtimes: it would add the runner to the
    # docker and podman groups, which is root-equivalent. Both are disabled
    # and the rootless socket is wired up below instead.
    runtimes = {
      docker = false;
      podman = false;
    };

    settings = {
      runner = {
        # Keep every job in a container; do not expose a host-execution label.
        labels = [
          "docker:${image}"
          "ubuntu-latest:${image}"
          "ubuntu-22.04:${image}"
        ];
        # One daemon can execute several jobs concurrently.
        capacity = 8;
        # Jobs get the runner's Podman socket (container.docker_host below),
        # so testcontainers works everywhere. It must connect through the
        # pasta mapping, and bind-mount the socket by its host path.
        envs = {
          TESTCONTAINERS_HOST_OVERRIDE = hostFromJobs;
          TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE = podmanSocket;
        };
      };
      # The internal cache server is only spoken to by the proxy and rejects
      # requests without the proxy's secret; pin it so it stops landing on a
      # random port.
      cache = {
        port = 4100;
        proxy_port = cacheProxyPort;
        actions_cache_url_override = "http://${hostFromJobs}:${toString cacheProxyPort}";
      };
      # Mount the socket the runner itself uses (DOCKER_HOST) into every job at
      # /var/run/docker.sock. It is the rootless user's own Podman, so a job can
      # reach other jobs' containers but nothing beyond that user.
      container.docker_host = "automount";
      server.connections.forgejo = {
        url = "https://git.bcnelson.dev/";
        uuid = "54f6999c-067c-4ce5-93d7-0f7b97cce5ab";
      };
    };
    secrets.server.connections.forgejo.token_url = config.age.secrets.forgejo_runner_token.path;
  };

  systemd.services.forgejo-runner-romeo = {
    requires = [ userManager ];
    after = [ userManager ];
    environment.DOCKER_HOST = "unix://${podmanSocket}";
    serviceConfig = {
      DynamicUser = lib.mkForce false;
      User = runnerUser;
      Group = runnerUser;
    };
  };

  # Testcontainers' reaper (Ryuk) is commonly disabled under Podman, so
  # containers from cancelled or crashed jobs can outlive them. Remove
  # testcontainers leftovers older than the runner's default 3h job timeout,
  # plus stopped containers and dangling images.
  systemd.services.forgejo-runner-podman-gc = {
    description = "Prune leftover Forgejo job containers";
    requires = [ userManager ];
    after = [ userManager ];
    path = [
      pkgs.podman
      pkgs.coreutils
    ];
    environment.XDG_RUNTIME_DIR = "/run/user/${toString runnerUid}";
    serviceConfig = {
      Type = "oneshot";
      User = runnerUser;
      Group = runnerUser;
    };
    script = ''
      cutoff=$(( $(date +%s) - 4 * 3600 ))
      for id in $(podman ps -aq --filter label=org.testcontainers=true); do
        created=$(podman inspect --format '{{.Created.Unix}}' "$id" 2>/dev/null) || continue
        if [ "$created" -lt "$cutoff" ]; then
          podman rm -f "$id"
        fi
      done
      podman container prune -f --filter until=4h
      podman image prune -f
      podman volume prune -f --filter until=24h
    '';
  };
  systemd.timers.forgejo-runner-podman-gc = {
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnCalendar = "hourly";
      RandomizedDelaySec = "10m";
    };
  };

  # The recovery specialisation deliberately starts no containers. Remove the
  # runner there too.
  specialisation.recovery.configuration.services.forgejo-runner.instances = lib.mkForce { };
}
