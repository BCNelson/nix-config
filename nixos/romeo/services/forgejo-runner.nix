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

  # The recovery specialisation deliberately disables Docker. Remove the
  # Docker-backed runner instance there as well so the module's runtime
  # assertion remains valid.
  specialisation.recovery.configuration.services.gitea-actions-runner.instances =
    lib.mkForce { };
}
