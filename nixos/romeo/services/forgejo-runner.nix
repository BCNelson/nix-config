{
  config,
  lib,
  pkgs,
  ...
}:
let
  labels = [
    "docker:docker://ghcr.io/catthehacker/ubuntu:act-22.04"
    "ubuntu-latest:docker://ghcr.io/catthehacker/ubuntu:act-22.04"
    "ubuntu-22.04:docker://ghcr.io/catthehacker/ubuntu:act-22.04"
  ];

  # Job containers reach the runner's cache proxy at ACTIONS_CACHE_URL, which
  # points at romeo's LAN IP. Pin the port so the firewall can admit it.
  cacheProxyPort = 4101;
in
{
  age.secrets.forgejo_runner_token.rekeyFile = ./secrets/forgejo_runner_token.age;

  services.gitea-actions-runner = {
    package = pkgs.forgejo-runner;
    instances.romeo = {
      enable = true;
      name = "romeo";
      url = "https://git.bcnelson.dev";
      # The NixOS module still requires a legacy registration token even when
      # the v12 server.connections configuration is used. Registration is
      # disabled below, so this non-secret value is never sent to Forgejo.
      token = "connection-configured";

      # One daemon can execute several jobs concurrently. Keep every job in a
      # container; do not expose a host-execution label.
      inherit labels;
      settings = {
        runner.capacity = 8;
        # The internal cache server is only spoken to by the proxy over
        # loopback; pin it too so it stops landing on a random port.
        cache = {
          port = 4100;
          proxy_port = cacheProxyPort;
        };
        server.connections.forgejo = {
          url = "https://git.bcnelson.dev/";
          uuid = "54f6999c-067c-4ce5-93d7-0f7b97cce5ab";
          token_url = "file:$CREDENTIALS_DIRECTORY/forgejo-token";
          inherit labels;
        };
      };
    };
  };

  # Forgejo Runner v12 accepts the UUID/token pair directly in its config.
  # Bypass the NixOS module's legacy registration step and provide the token as
  # a systemd credential, keeping it out of both the Nix store and environment.
  systemd.services.gitea-runner-romeo.serviceConfig = {
    ExecStartPre = lib.mkForce [ ];
    LoadCredential = "forgejo-token:${config.age.secrets.forgejo_runner_token.path}";
  };

  # romeo enables both Docker and Podman, and the module prefers Podman's
  # socket. Podman 5.8.7 (buildah 1.43.4, CVE-2026-79705 hardening) rejects
  # archive PUTs that cross absolute symlinks like /var/run, which breaks every
  # job's file copy into the container: containers/podman#29805, fix pending in
  # containers/buildah#7129. Docker shipped the same regression and fixed it in
  # 29.5.2 (moby#53258), so run jobs on Docker. Revisit once a fixed Podman lands.
  systemd.services.gitea-runner-romeo.environment.DOCKER_HOST =
    lib.mkForce "unix:///run/docker.sock";

  # Each job runs on its own runner-created Docker bridge (br-<random>), so
  # match on Docker's default 172.16.0.0/12 address pool rather than an
  # interface name. Without this, actions/cache times out with ETIMEDOUT.
  networking.firewall.extraCommands = ''
    iptables -I nixos-fw -s 172.16.0.0/12 -p tcp --dport ${toString cacheProxyPort} -j nixos-fw-accept
  '';
  networking.firewall.extraStopCommands = ''
    iptables -D nixos-fw -s 172.16.0.0/12 -p tcp --dport ${toString cacheProxyPort} -j nixos-fw-accept 2>/dev/null || true
  '';

  # The recovery specialisation deliberately disables Docker. Remove the
  # Docker-backed runner instance there as well so the module's runtime
  # assertion remains valid.
  specialisation.recovery.configuration.services.gitea-actions-runner.instances =
    lib.mkForce { };
}
