{ config, ... }:
let
  # Ephemeral CI build cache shared by every pipeline step this host builds,
  # mounted at /cache. Pipelines namespace their own state under a subfolder
  # (Rust uses /cache/rust for sccache's content-addressed compiler-output cache
  # plus the cargo registry) so unrelated tooling can share the mount as siblings.
  # sccache is concurrency-safe, so all WOODPECKER_MAX_WORKFLOWS builds
  # across BOTH agents can share one cache dir without the cargo-lock contention a
  # shared `target/` would cause — which is why pipelines use sccache and a
  # per-build (ephemeral) target rather than persisting `target/`. Bounded on the
  # sccache side by SCCACHE_CACHE_SIZE (set in each pipeline); the cargo registry
  # alongside it is small. Lives on the level7 "ephemeral" tier as it is fully
  # regenerable — safe to wipe.
  #
  # Mounted into EVERY step of both agents via WOODPECKER_BACKEND_DOCKER_VOLUMES
  # (an agent-level mount — it needs no "trusted repo" flag, which Codeberg's
  # shared server won't grant). Any pipeline on either agent can therefore
  # read/write it; that's fine because both the home-first org and the personal
  # account are ours. Don't point an agent that builds untrusted third-party
  # repos at this cache.
  ciCacheHost = "${config.data.dirs.level7}/woodpecker-ci";
  ciCacheVolume = "${ciCacheHost}:/cache";
in
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

  # An agent may belong to only ONE user/org, so each Codeberg org/user romeo
  # contributes to needs its own registered agent with its own token.

  # Raw agent tokens from Codeberg. Manually entered (not generated): create each
  # with `agenix edit nixos/romeo/services/secrets/<file>.age`, paste the token
  # from Codeberg, then `just rekey` and `git add`.
  #   - woodpecker_agent_secret.age          -> home-first org (ci.codeberg.org/orgs/2684)
  #   - woodpecker_agent_secret_personal.age -> personal account (bcnelson)
  age.secrets.woodpecker_agent_secret.rekeyFile = ./secrets/woodpecker_agent_secret.age;
  age.secrets.woodpecker_agent_secret_personal.rekeyFile = ./secrets/woodpecker_agent_secret_personal.age;

  # woodpecker-agent reads its secret from an env file (KEY=value). Compose that
  # file from the raw token, mirroring the acme env-file pattern in default.nix.
  age-template.files."woodpecker-agent-env" = {
    vars.token = config.age.secrets.woodpecker_agent_secret.path;
    content = "WOODPECKER_AGENT_SECRET=$token";
  };
  age-template.files."woodpecker-agent-env-personal" = {
    vars.token = config.age.secrets.woodpecker_agent_secret_personal.path;
    content = "WOODPECKER_AGENT_SECRET=$token";
  };

  # World-writable so a step container running as any UID (rust:bookworm runs as
  # root, others may not) can populate the cache.
  systemd.tmpfiles.rules = [
    "d ${ciCacheHost} 0777 root root - -"
  ];

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
      # Mount the shared build cache into every step container (see ciCacheVolume).
      WOODPECKER_BACKEND_DOCKER_VOLUMES = ciCacheVolume;
    };
    # The docker backend drives builds through the host docker socket.
    extraGroups = [ "docker" ];
    environmentFile = [ config.age-template.files."woodpecker-agent-env".path ];
  };

  # Second agent, bound to the personal Codeberg account (bcnelson). Same
  # Codeberg server and local docker backend as the home-first agent above; only
  # the token (and the hostname label) differ.
  services.woodpecker-agents.agents.personal = {
    enable = true;
    environment = {
      WOODPECKER_SERVER = "grpc.ci.codeberg.org:443";
      WOODPECKER_GRPC_SECURE = "true";
      WOODPECKER_BACKEND = "docker";
      WOODPECKER_HOSTNAME = "romeo-personal";
      WOODPECKER_MAX_WORKFLOWS = "4";
      WOODPECKER_BACKEND_DOCKER_VOLUMES = ciCacheVolume;
    };
    extraGroups = [ "docker" ];
    environmentFile = [ config.age-template.files."woodpecker-agent-env-personal".path ];
  };
}
