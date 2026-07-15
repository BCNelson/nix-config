{ config, ... }:
{
  # Self-hosted Woodpecker CI agent that connects out to Codeberg's hosted
  # Woodpecker server (ci.codeberg.org) to run pipelines for the home-first org
  # (https://ci.codeberg.org/orgs/2684). Codeberg runs the server; romeo only
  # contributes build capacity as a registered agent.
  #
  # The agent authenticates with a per-agent token generated on Codeberg at
  # Org -> Settings -> Agents -> "add agent". That token is the
  # WOODPECKER_AGENT_SECRET below. An agent may belong to only ONE user/org, so
  # this one is bound to the home-first org.

  # Raw agent token from Codeberg. Manually entered (not generated): create it
  # with `agenix edit nixos/romeo/services/secrets/woodpecker_agent_secret.age`,
  # paste the token from Codeberg, then `just rekey` and `git add`.
  age.secrets.woodpecker_agent_secret.rekeyFile = ./secrets/woodpecker_agent_secret.age;

  # woodpecker-agent reads its secret from an env file (KEY=value). Compose that
  # file from the raw token, mirroring the acme env-file pattern in default.nix.
  age-template.files."woodpecker-agent-env" = {
    vars.token = config.age.secrets.woodpecker_agent_secret.path;
    content = "WOODPECKER_AGENT_SECRET=$token";
  };

  services.woodpecker-agents.agents.codeberg = {
    enable = true;
    environment = {
      # Codeberg CI's gRPC endpoint (see https://docs.codeberg.org/ci/agents/).
      WOODPECKER_SERVER = "grpc.ci.codeberg.org:443";
      WOODPECKER_GRPC_SECURE = "true";
      # Run pipeline steps as containers on romeo's local docker daemon. Default
      # DOCKER_HOST (unix:///var/run/docker.sock) is correct here.
      WOODPECKER_BACKEND = "docker";
      # Friendly name shown on Codeberg's agents list.
      WOODPECKER_HOSTNAME = "romeo";
      # Number of workflows this agent runs in parallel (default 1).
      WOODPECKER_MAX_WORKFLOWS = "4";
    };
    # The docker backend drives builds through the host docker socket.
    extraGroups = [ "docker" ];
    environmentFile = [ config.age-template.files."woodpecker-agent-env".path ];
  };
}
