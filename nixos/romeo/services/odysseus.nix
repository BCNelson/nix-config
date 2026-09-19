{ config, pkgs, lib, ... }:
let
  dataDirs = config.data.dirs;

  port = 7000; # upstream's default; nothing else on romeo wants it
  domain = "odysseus.h.b.nel.family";
  adminUser = "bcnelson";

  # Everything Odysseus persists lives under one directory it calls DATA_DIR:
  # the SQLite database, sessions, memory, notes, documents, mail attachments,
  # the agent workspace, and the .app_key that encrypts stored credentials. That
  # is user-authored content, so it goes on level3 (High) and rides the borg job
  # in ../backups.nix rather than sitting on the root pool.
  stateDir = "${dataDirs.level3}/odysseus";

  # FastEmbed pulls ONNX encoder weights from HuggingFace on first use and
  # caches them here. Hundreds of megabytes, re-downloadable, and it would
  # otherwise default to a subdirectory of DATA_DIR -- i.e. straight into the
  # level3 backups. level7 (/cache) is where that belongs.
  fastembedCache = "${dataDirs.level7}/odysseus/fastembed";

  # ../services/searxng.nix, which already has `formats: [html json]` enabled
  # for openclaw's web_search tool. Odysseus talks the same JSON API, so this
  # reuses that instance instead of standing up the searxng sidecar from
  # upstream's docker-compose.yml.
  searxPort = 8888;

  # ChromaDB via nixpkgs' services.chromadb (below). 8100 rather than the NixOS
  # module's default of 8000, which ../docker-services/defs/paperless.nix
  # already answers on -- and 8100 is the host port upstream's compose file
  # publishes it on anyway.
  chromaPort = 8100;

  # setup.py's DIRS list, minus the one entry that is not under DATA_DIR
  # (BASE_DIR/logs -- app.py actually logs to DATA_DIR/logs) and plus the
  # fastembed cache. The app creates most of these lazily, but not all of them,
  # and we are not running setup.py -- see the note in ../../../pkgs/odysseus-ai.
  dataSubdirs = [
    "logs"
    "uploads"
    "personal_docs"
    "personal_uploads"
    "tts_cache"
    "generated_images"
    "deep_research"
    "chroma"
    "rag"
    "memory_vectors"
    "agent_workspace"
  ];
in
{
  # The password the human types at the login form. Odysseus keeps its own
  # bcrypt hash of it in DATA_DIR/auth.json, so this is only read once, by the
  # seeding unit below, on an install that has no auth.json yet. Same shape as
  # ./node-red.nix's admin password: generated, never chosen, and synced to
  # Bitwarden because someone has to be able to look it up.
  age.secrets.odysseus-admin-password = {
    rekeyFile = ./secrets/odysseus_admin_password.age;
    generator.script = "passphrase";
    bitwarden = {
      name = "Odysseus Admin Password";
      username = adminUser;
      uris = { uri = "https://${domain}"; matchType = "host"; };
    };
  };

  # Upstream bootstraps the admin account from setup.py, which the package does
  # not install (it writes into the app root, which is the read-only store). The
  # part of it that matters is small enough to restate: auth.json with one
  # bcrypt-hashed admin. core/auth.py verifies with bcrypt.checkpw and
  # normalises a record that carries no `privileges` key, which is exactly the
  # shape setup.py writes, so this is the same file upstream would have written.
  #
  # Guarded on the file not existing, and that guard is load-bearing: accounts,
  # password changes and per-user privileges all live in this file once the
  # instance is in use. Re-rendering it on every start -- the way
  # ./node-red.nix's env file is -- would silently revert all of that.
  #
  # mkpasswd rather than the python bcrypt in the package's environment: it
  # emits the same $2b$ hash and keeps the seeder to a shell script.
  systemd.services.odysseus-seed-admin = {
    description = "Seed the Odysseus admin account on first start";
    before = [ "odysseus.service" ];
    requiredBy = [ "odysseus.service" ];
    # auth.json lands on the vault, so the same rule the app itself follows
    # applies here: without this the "does it already exist?" check would run
    # against the bare mountpoint, decide the instance is new, and write a fresh
    # auth.json underneath the real one.
    requires = [ "zfs-import.target" ];
    after = [ "zfs-import.target" ];
    path = [ pkgs.mkpasswd pkgs.jq pkgs.coreutils ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = false;
    };
    script = ''
      set -euo pipefail

      auth="${stateDir}/auth.json"
      if [ -e "$auth" ]; then
        echo "auth.json already exists, leaving accounts alone"
        exit 0
      fi

      hash="$(mkpasswd -m bcrypt -R 12 "$(cat ${config.age.secrets.odysseus-admin-password.path})")"

      umask 077
      jq -n --arg u "${adminUser}" --arg h "$hash" \
        '{users: {($u): {password_hash: $h, is_admin: true}}}' > "$auth.tmp"
      chown odysseus:odysseus "$auth.tmp"
      mv "$auth.tmp" "$auth"
      echo "seeded admin account '${adminUser}'"
    '';
  };

  # The vector store Odysseus uses for RAG, semantic memory and tool selection.
  # It is not optional in the way the other sidecars are: src/chroma_client.py
  # has no embedded/persistent-client fallback at all -- it probes the TCP port
  # and raises "ChromaDB is not reachable" if nothing answers, so RAG and memory
  # vectors are simply dead without a server.
  #
  # This is the part of the deployment that *is* in nixpkgs, module and all, so
  # upstream's chromadb container has no reason to exist here.
  services.chromadb = {
    enable = true;
    host = "127.0.0.1";
    port = chromaPort;
    # Left on the module default (/var/lib/chromadb) rather than moved to the
    # vault, for the same reason ./librechat.nix leaves the Meilisearch index
    # there: the contents are embeddings derived from the documents and memory
    # under ${stateDir}, which is what the backups actually protect. Losing this
    # costs a re-index, not data. Note the module runs the unit with
    # DynamicUser + ProtectSystem=strict, so pointing dbpath at /mnt/vault would
    # need ReadWritePaths plumbing to go with it.
  };

  systemd.tmpfiles.rules = [
    "d ${stateDir} 0700 odysseus odysseus - -"
    "d ${dataDirs.level7}/odysseus 0700 odysseus odysseus - -"
    "d ${fastembedCache} 0700 odysseus odysseus - -"
    "d ${dataDirs.level7}/odysseus/playwright-mcp-cache 0700 odysseus odysseus - -"
  ] ++ map (d: "d ${stateDir}/${d} 0700 odysseus odysseus - -") dataSubdirs;

  systemd.services.odysseus = {
    description = "Odysseus AI workspace";
    wantedBy = [ "multi-user.target" ];

    # zfs-import is required, not merely ordered: ${stateDir} is on the vault and
    # a start before the pool is imported would have the app create a fresh
    # SQLite database on the mountpoint directory underneath it.
    requires = [ "zfs-import.target" ];
    after = [ "zfs-import.target" "network-online.target" "chromadb.service" "searx.service" "ollama.service" ];
    # The rest are wants: model discovery, web search and the vector store are
    # all resolved per request rather than at startup, so coming up before any
    # of them costs a failed first search or an empty model picker, not a dead
    # instance.
    wants = [ "network-online.target" "chromadb.service" "searx.service" "ollama.service" ];

    environment = {
      # app.py's __main__ reads these two and hands them to uvicorn.run(). nginx
      # below is the only way in; do not move APP_BIND off loopback.
      APP_BIND = "127.0.0.1";
      APP_PORT = toString port;

      # The single knob for where state lives -- src/constants.py reads it in
      # exactly one place and derives every persisted path from it.
      ODYSSEUS_DATA_DIR = stateDir;
      FASTEMBED_CACHE_PATH = fastembedCache;
      HOME = stateDir;

      AUTH_ENABLED = "true";
      # Upstream's own security note: keep this false outside local dev. It
      # bypasses authentication for loopback requests, and nginx *is* a loopback
      # peer, so turning it on would publish an unauthenticated workspace to
      # everything the vhost lets through.
      LOCALHOST_BYPASS = "false";
      # The documented knob for a TLS-terminating proxy. Without it the Secure
      # attribute is inferred per request, which works here but leaves the
      # cookie's scope depending on a header.
      SECURE_COOKIES = "true";
      ALLOWED_ORIGINS = "https://${domain}";
      # MCP OAuth callbacks are built from this. The app only ever sees
      # 127.0.0.1:7000 and cannot know the public name, so a remote MCP server's
      # redirect would come back to a URL that does not resolve.
      OAUTH_REDIRECT_BASE_URL = "https://${domain}";

      CHROMADB_HOST = "127.0.0.1";
      CHROMADB_PORT = toString chromaPort;
      SEARXNG_INSTANCE = "http://127.0.0.1:${toString searxPort}";

      # Populates the model picker from ./ollama.nix's catalog. ./cli-proxy-api.nix
      # (the ChatGPT/Codex credential ./librechat.nix uses) is deliberately not
      # wired here: Odysseus configures OpenAI-compatible endpoints in its
      # Settings UI, stored in ${stateDir}/settings.json, not from the
      # environment. Add it there as http://127.0.0.1:8317/v1 with the key from
      # /run/agenix/cli-proxy-api-key if wanted.
      OLLAMA_BASE_URL = "http://127.0.0.1:11434";

      # Embeddings for RAG, semantic memory and tool selection. src/embeddings.py
      # tries an HTTP endpoint first and falls back to local FastEmbed (ONNX),
      # and Ollama is its intended first choice -- the module docstring names
      # `EMBEDDING_URL=http://localhost:11434/v1/embeddings (ollama)` and the URL
      # below is only spelled out because the default builds it from LLM_HOST.
      #
      # EMBEDDING_MODEL is the part that has to be set. It defaults to
      # "all-minilm:l6-v2", which ./ollama.nix does not load, and Ollama answers
      # an unpulled model with
      #   404 {"message": "model \"all-minilm:l6-v2\" not found, try pulling it first"}
      # which src/embeddings.py catches as "HTTP embedding API unavailable" and
      # then silently serves every embedding from FastEmbed instead. Naming the
      # encoder ./ollama.nix already keeps resident avoids both the 404 and a
      # second embedding model on the box. Verified against romeo's ollama:
      # /v1/embeddings returns 768-dim vectors for this model.
      #
      # One difference from openclaw's use of the same model, and it is harmless:
      # openclaw's adapter prepends "search_query: " to queries, which nomic was
      # trained to expect. This OpenAI-compatible path does no such templating,
      # so documents and queries are both embedded bare -- asymmetric prefixing
      # would retrieve slightly better, but consistency between the two sides is
      # what actually matters and that holds.
      #
      # Changing this model later is safe and needs no manual re-index, which is
      # worth knowing before anyone hesitates over it. src/embedding_lanes.py
      # keeps one Chroma collection per encoder ("<base>_custom" for this
      # endpoint, "<base>_fastembed" for the fallback) because Chroma fixes a
      # collection's dimension on first insert, so 768 and 384 never collide. It
      # stamps each collection with a fingerprint of lane/url/model/dimension,
      # and on a mismatch it preserves the documents, deletes the collection,
      # re-embeds them with the new model, and restores the old vectors if that
      # write fails. Startup logs it as "Recreating Chroma collection ... for
      # embedding lane change".
      EMBEDDING_URL = "http://127.0.0.1:11434/v1/embeddings";
      EMBEDDING_MODEL = "nomic-embed-text";

      # The built-in Browser MCP server, turned off deliberately -- and this is
      # the one place the read-only store actually bites. src/builtin_mcp.py
      # defaults its Playwright cache to os.path.join(base_dir, "data", "local",
      # "playwright-mcp-cache") and calls os.makedirs on it. base_dir is the app
      # root, not ODYSSEUS_DATA_DIR, so on a store-installed copy startup logs
      #   Built-in NPX server Built-in: Browser error: OSError: [Errno 30]
      #   Read-only file system: '/nix/store/...-odysseus-ai-.../share/odysseus/data'
      #
      # REQUIRE_CACHE=1 makes it check the npx cache first and skip with an
      # explanatory warning instead, which is what we want regardless of the
      # error: the default path is `npx -y @playwright/mcp@latest`, i.e. an
      # unpinned npm install from the network on every single start, followed by
      # a Playwright browser download. The package never excludes chromium from
      # the closure and then lets the app fetch its own, and there is no browser
      # in PATH for it to drive anyway.
      #
      # The cache path is still redirected somewhere writable so that opting
      # back in (drop REQUIRE_CACHE, add chromium to the package's runtimePath)
      # lands the download on /cache rather than failing on the store again.
      ODYSSEUS_BROWSER_MCP_REQUIRE_CACHE = "1";
      ODYSSEUS_BROWSER_MCP_CACHE = "${dataDirs.level7}/odysseus/playwright-mcp-cache";
    };

    serviceConfig = {
      ExecStart = lib.getExe pkgs.odysseus-ai;
      User = "odysseus";
      Group = "odysseus";
      WorkingDirectory = stateDir;
      Restart = "on-failure";
      RestartSec = "10s";

      # Same moderate hardening as ./openclaw.nix and ./goose.nix, and for the
      # same reason: the agent's whole job is to run shell commands, read and
      # write files, and launch MCP subprocesses, so the aggressive filters used
      # on ./cli-proxy-api.nix would break it. What is kept still blocks
      # privilege escalation and confines writes.
      #
      # ProtectSystem only stops *writes*. A prompt-injected agent here can
      # still read /mnt/vault and relay what it reads to whichever model the
      # session is pointed at; the vhost's allowlist and the login are what bound
      # who can start such a session. ./goose.nix documents why InaccessiblePaths=
      # was rejected as the fix (it masks mounts with tmpfs and corrupts `df`).
      ReadWritePaths = [ stateDir "${dataDirs.level7}/odysseus" ];
      NoNewPrivileges = true;
      PrivateTmp = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      ProtectKernelTunables = true;
      ProtectKernelModules = true;
      ProtectControlGroups = true;
      RestrictRealtime = true;
      RestrictSUIDSGID = true;
      LockPersonality = true;
      SystemCallArchitectures = "native";
      RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
    };
  };

  users.users.odysseus = {
    isSystemUser = true;
    group = "odysseus";
    home = stateDir;
  };
  users.groups.odysseus = { };

  services.nginx.virtualHosts."${domain}" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      # Uploads (documents, gallery images, mail attachments) are capped by the
      # app's own ODYSSEUS_*_MAX_BYTES settings, not by nginx.
      client_max_body_size 0;

      # Same allowlist as ./librechat.nix. This is not a service to publish: it
      # hands an LLM agent a shell, a file tool and the host's network, so the
      # login is the second line of defence rather than the only one.
      #Allow access from Tailscale network
      allow 100.64.0.0/10;
      #Allow access from local network
      allow 192.168.0.0/16;
      deny all;
    '';
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString port}";
      proxyWebsockets = true;
      extraConfig = ''
        # Deep research and a long generation both run well past the http-level
        # 60s default, and streamed tokens need buffering off or nginx holds
        # them until the response completes.
        proxy_read_timeout 600s;
        proxy_send_timeout 600s;
        proxy_buffering off;
      '';
    };
  };
}
