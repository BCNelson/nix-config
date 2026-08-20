{
  config,
  pkgs,
  lib,
  ...
}: let
  port = 18789; # openclaw gateway default
  domain = "openclaw.h.b.nel.family";
  stateDir = "/var/lib/openclaw";
  workspace = "${stateDir}/workspace";

  # ./matrix.nix on this same host. Both of the accounts named here are
  # provisioned by its `accounts` roster, and the gateway's password is the
  # same agenix secret that roster created it with -- so there is exactly one
  # source of truth for the credential and nothing to mint by hand.
  matrixServerName = "bot.nel.family";

  # The Matrix IDs allowed to talk to the agent, and the only ones it will
  # answer. These are plain allowlist entries, so a wrong value here fails
  # closed: the bot joins the DM and then silently ignores every message in it.
  #
  # Display names are deliberately not matchable (channels.matrix has a
  # dangerouslyAllowNameMatching escape hatch precisely because display names
  # are mutable and therefore spoofable); each of these has to be the full
  # @user:server MXID.
  #
  # An MXID is per-homeserver, not per-person: the same human on a different
  # server is a different principal here and has to be listed separately. The
  # matrix.org entry is the pre-existing account, reaching this homeserver over
  # federation -- which works because ./matrix.nix sets allow_federation and
  # lists matrix.org in trusted_servers. Federated DMs are still E2EE, so this
  # costs nothing in confidentiality, but it does mean the allowlist now
  # depends on matrix.org's account security as well as this host's.
  ownerMxids = [
    "@bcnelson:${matrixServerName}"
    "@bcnelson:matrix.org"
  ];

  agentTools = [pkgs.bash pkgs.coreutils pkgs.git pkgs.ripgrep pkgs.jq pkgs.curl];

  # E2EE is not purely declarative: cross-signing, device verification and the
  # room-key backup are all *state*, bootstrapped by `openclaw matrix ...`
  # commands that have to see the same config, state dir and access token the
  # daemon does. `sudo -u openclaw openclaw ...` does not -- it inherits none
  # of the unit's environment, so it silently reads a nonexistent
  # ~/.openclaw/openclaw.json and reports a gateway with no channels.
  #
  # systemd-run reconstructs that environment from the unit's own definition
  # rather than a second copy of it, so the two cannot drift, and
  # EnvironmentFile is read by systemd as root instead of being catted into an
  # argv where the token would be visible in ps output. --pipe rather than
  # --pty because the openclaw CLI hangs waiting on a controlling terminal that
  # a transient unit does not have.
  #
  # Usage: sudo openclaw-admin matrix encryption setup
  #        sudo openclaw-admin doctor
  #        printf '%s\n' "$KEY" | sudo openclaw-admin matrix verify device --recovery-key-stdin
  #
  # The crypto store is a sqlite file the running gateway holds open, so if a
  # verification command reports it as locked, `systemctl stop openclaw` first.
  openclawAdmin = pkgs.writeShellApplication {
    name = "openclaw-admin";
    runtimeInputs = [pkgs.systemd];
    text = ''
      exec systemd-run --pipe --quiet --wait --collect \
        --uid=openclaw --gid=openclaw \
        --property=WorkingDirectory=${workspace} \
        --property=EnvironmentFile=${config.age-template.files.openclaw-env.path} \
        --setenv=PATH=${lib.makeBinPath agentTools} \
        ${
        lib.concatMapStringsSep " \\\n        " (
          name: "--setenv=${name}=${config.systemd.services.openclaw.environment.${name}}"
        ) (builtins.attrNames config.systemd.services.openclaw.environment)
      } \
        ${lib.getExe openclaw} "$@"
    '';
  };

  # pkgs/openclaw.nix, not nixpkgs' openclaw: nixpkgs tracks the 2026.6
  # maintenance line, which never received the `/pair qr` "Media failed" fix
  # (openclaw/openclaw#97933). The `additions` overlay shadows the nixpkgs
  # attribute, so `pkgs.openclaw` here is ours. Version, hashes and the reason
  # for dropping the insecure marker are all documented there.
  inherit (pkgs) openclaw;

  # OPENCLAW_NIX_MODE is what makes this file, rather than the running daemon,
  # the source of truth. Without it openclaw treats openclaw.json as mutable and
  # rewrites it atomically (rename onto the path) during onboarding, `config
  # set`, `doctor --fix` and plugin install -- which would either clobber a
  # store path or, for a symlink, replace the *target*. Nix mode makes the
  # config immutable and disables every self-mutation flow, so pointing
  # OPENCLAW_CONFIG_PATH straight at a read-only store file is safe.
  #
  # Consequence worth knowing: `openclaw config set ...` and `openclaw plugins
  # install ...` will refuse to run. Editing this file and rebuilding is the
  # only way to change configuration, which is the point.
  #
  # Startup logs one harmless warning as a result:
  #   failed to promote config last-known-good backup: EROFS: read-only file
  #   system, open '/nix/store/...-openclaw.json.last-good'
  # openclaw keeps a .last-good sidecar next to the config to roll back a bad
  # self-edit. There are no self-edits to roll back here and the store is
  # read-only, so the warning is expected and the gateway continues to "ready".

  # JSON is a strict subset of the JSON5 the gateway parses, so the normal
  # generator is fine -- no custom writer needed.
  openclawConfig = (pkgs.formats.json {}).generate "openclaw.json" {
    gateway = {
      inherit port;
      # Mandatory, and easy to miss: the gateway refuses to start with
      # "Gateway start blocked: set gateway.mode=local" when this key is absent.
      # Normally `openclaw onboard` stamps it, but Nix mode disables onboarding
      # and every other config writer, so it has to be declared here.
      mode = "local";
      # nginx terminates TLS and owns the allowlist; the gateway itself only
      # ever needs to answer on loopback. "tailnet" would bind the tailscale
      # address directly and bypass the vhost's allow/deny rules.
      bind = "loopback";
      # Interpolated from the process environment at config load time, so the
      # token never lands in the world-readable store. Only uppercase
      # [A-Z_][A-Z0-9_]* names are substituted, and a missing/empty var is a
      # hard error at startup rather than a silently unauthenticated gateway.
      auth.token = "\${OPENCLAW_GATEWAY_TOKEN}";

      # Without this the Control UI loads but the gateway rejects its WebSocket
      # with "Browser origin not allowed".
      #
      # The gateway auto-trusts same-origin UI loads from loopback, RFC1918,
      # link-local, *.local, *.ts.net and Tailscale CGNAT hosts -- which is why
      # this is not needed for a plain http://<lan-ip>:18789 load. It IS needed
      # here because the vhost is reached by a public DNS name
      # (openclaw.h.b.nel.family), and openclaw classifies the origin by the
      # name in the browser's address bar, not by the address it resolves to.
      # nginx being LAN/tailnet-only makes no difference to that check.
      #
      # Exact origin match: scheme + host, no trailing slash, no port (443 is
      # implicit for https). The alternative upstream offers is
      # dangerouslyAllowHostHeaderOriginFallback, which trusts the Host header
      # an attacker controls -- an explicit allowlist is the safe form.
      controlUi.allowedOrigins = ["https://${domain}"];
    };

    models.providers.cliproxy = {
      # Same cli-proxy-api that librechat.nix and goose.nix already talk to: it
      # holds the ChatGPT OAuth credential and presents an OpenAI-compatible
      # surface. Note this is a *custom* provider id rather than the built-in
      # `openai` one -- upstream routes `openai/*` agent turns through the
      # native Codex app-server runtime, which wants real Codex auth and would
      # ignore baseUrl entirely. A custom `openai-completions` provider is the
      # documented way to point at a local OpenAI-compatible endpoint.
      baseUrl = "http://127.0.0.1:8317/v1";
      apiKey = "\${CLI_PROXY_API_KEY}";
      api = "openai-completions";
      # Requests to private addresses are guarded by default. A custom provider
      # implicitly trusts its own baseUrl origin, but state this explicitly so a
      # future port change does not silently start failing the guard.
      request.allowPrivateNetwork = true;
      # Catalog must match what cli-proxy-api actually serves. Verified against
      # its /v1/models on romeo: gpt-5.4, gpt-5.4-mini, gpt-5.5, gpt-5.6-{sol,
      # luna,terra}, plus image/codex entries. There is no plain "gpt-5".
      #
      # Every field below is REQUIRED by openclaw's schema for a custom
      # provider. Omitting them fails startup with
      # "models.providers.cliproxy.models.N.name: Invalid input" -- the schema
      # reports only the first missing key per entry, so a partial fix just
      # surfaces the next one. Built-in providers fetch this metadata from a
      # remote catalog; a custom provider has to declare it inline.
      #
      # cost is 0 across the board because this rides the ChatGPT subscription
      # through cli-proxy-api rather than per-token API billing -- these numbers
      # only feed openclaw's spend reporting, and a nonzero rate here would
      # invent charges that do not exist. contextWindow/maxTokens are the
      # published GPT-5-family figures; they drive compaction and session
      # budgeting only, so adjust them if a model's real limits differ.
      models =
        map (m: {
          inherit (m) id name;
          reasoning = true;
          input = ["text" "image"];
          cost = {
            input = 0;
            output = 0;
            cacheRead = 0;
            cacheWrite = 0;
          };
          contextWindow = 400000;
          maxTokens = 128000;
        }) [
          {
            id = "gpt-5.5";
            name = "GPT-5.5";
          }
          {
            id = "gpt-5.4";
            name = "GPT-5.4";
          }
          {
            id = "gpt-5.4-mini";
            name = "GPT-5.4 mini";
          }
        ];
    };

    # ../ollama.nix on this same host, reached over loopback exactly like every
    # other consumer (librechat.nix, tendant.nix, goose.nix).
    #
    # The models are listed explicitly, which does switch openclaw off live
    # discovery for this provider -- and that is the point. Discovery only ever
    # runs on CLI paths. The gateway deliberately serves models.list from a
    # read-only, side-effect-free catalog ("Keep gateway models.list on
    # side-effect-free sources. The RPC timeout cannot fire while provider
    # discovery blocks the event loop", src/agents/model-catalog.ts), so a
    # provider with no `models` array gets persisted to
    # <state>/agents/main/agent/plugins/ollama/catalog.json as `"models": []`
    # and every gateway client -- Control UI, phone app, TUI picker, agent
    # model refs -- sees an ollama provider with nothing in it.
    #
    # `openclaw models list --provider ollama` is what made the discovery-only
    # version look verified: that path probes /api/tags and /api/show live and
    # happily prints all ten. The gateway path never probes, and
    # `openclaw models list --gateway` showed only the cliproxy models.
    #
    # So this list has to be kept in sync with services.ollama.loadModels by
    # hand. The schema is all-or-nothing per entry: omit a field and startup
    # fails with "models.providers.ollama.models.N.<key>: Invalid input", one
    # key at a time, exactly as documented on the cliproxy block above.
    #
    # deepseek-coder:6.7b is preloaded but deliberately absent here: /api/show
    # reports only `completion` for it -- no tools, no thinking -- so an agent
    # turn on it could not call a single tool. Every other preloaded model
    # reports both.
    #
    # contextWindow is 32k rather than each model's real ceiling (262k on the
    # qwen3.5 and gemma4 lines, 131k on deepseek-r1). It feeds compaction and
    # session budgeting, and the B580 only offers ~10.2 GiB for weights plus KV
    # cache, so a session allowed to grow toward 262k would push the cache off
    # the GPU mid-conversation. 32k is measured to fit with room to spare:
    # gemma4:12b still holds 49/49 layers on the GPU at 64k, because only 8 of
    # its 48 layers keep a full KV cache and the rest are sliding-window.
    #
    # cost is zero across the board because this is local inference; the
    # numbers only feed openclaw's spend reporting.
    models.providers.ollama = {
      # No /v1 suffix. That path selects openclaw's OpenAI-compatible mode,
      # where upstream documents tool calling as unreliable -- models emit raw
      # tool-call JSON as assistant text instead. The bare origin selects the
      # native /api/chat API, which is what `api = "ollama"` below pins.
      baseUrl = "http://127.0.0.1:11434";
      # A documented non-secret marker, not a credential: the ollama plugin
      # lists "ollama-local" in its nonSecretAuthMarkers, and openclaw accepts
      # it for loopback/LAN/.local hosts instead of reporting a missing key.
      # Safe in the world-readable store for the same reason -- ollama itself
      # is unauthenticated on loopback, so there is nothing here to leak.
      apiKey = "ollama-local";
      # Redundant against the built-in provider default, but stated so a future
      # upstream change of default cannot quietly move this onto the /v1 path.
      api = "ollama";
      # input mirrors what /api/show reports per model: the gemma4 pair take
      # audio as well as images, the qwen3.5 line is text+image, and qwen3:4b
      # and the deepseek-r1 line are text only. reasoning is true for all ten
      # -- every one of them reports the `thinking` capability.
      models =
        map (m: {
          inherit (m) id name;
          reasoning = true;
          input = m.input or ["text"];
          cost = {
            input = 0;
            output = 0;
            cacheRead = 0;
            cacheWrite = 0;
          };
          contextWindow = 32768;
          maxTokens = 8192;
        }) [
          {
            id = "gemma4:12b";
            name = "Gemma 4 12B";
            input = ["text" "image" "audio"];
          }
          {
            id = "gemma4:12b-it-qat";
            name = "Gemma 4 12B (QAT)";
            input = ["text" "image" "audio"];
          }
          {
            id = "qwen3.5:9b";
            name = "Qwen 3.5 9B";
            input = ["text" "image"];
          }
          {
            id = "qwen3.5:4b";
            name = "Qwen 3.5 4B";
            input = ["text" "image"];
          }
          {
            id = "qwen3.5:2b";
            name = "Qwen 3.5 2B";
            input = ["text" "image"];
          }
          {
            id = "qwen3.5:0.8b";
            name = "Qwen 3.5 0.8B";
            input = ["text" "image"];
          }
          {
            id = "qwen3:4b";
            name = "Qwen 3 4B";
          }
          {
            id = "deepseek-r1:8b";
            name = "DeepSeek-R1 8B";
          }
          {
            id = "deepseek-r1:7b";
            name = "DeepSeek-R1 7B";
          }
          {
            id = "deepseek-r1:1.5b";
            name = "DeepSeek-R1 1.5B";
          }
        ];
    };

    agents.defaults = {
      inherit workspace;
      model.primary = "cliproxy/gpt-5.5";

      # What memory_search retrieves with. Unset, this defaults to OpenAI
      # embeddings, finds no OPENAI_API_KEY, and quietly degrades to keyword-only
      # BM25 -- which still answers, so nothing looks broken. It matters more
      # than it sounds: dreaming's deep phase ranks promotion candidates on
      # retrieval quality and query diversity (0.30 + 0.15 of the score), and
      # both are measured from recall hits. Lexical-only recall means only
      # material whose wording repeats exactly ever accumulates enough signal to
      # reach MEMORY.md.
      #
      # `ollama` here is the built-in adapter, and it borrows baseUrl and apiKey
      # from models.providers.ollama below -- there is no second endpoint to keep
      # in sync. `model` is resolved by the embedding adapter directly and is
      # deliberately absent from that provider's `models` catalog, which is the
      # chat picker; see the note in ../ollama.nix for why nomic-embed-text.
      #
      # Failure mode to know about: an explicitly named non-`none` provider
      # fails CLOSED. If ollama is down, memory_search returns "unavailable"
      # rather than falling back to BM25. That is the right trade here (silent
      # quality loss is worse than a visible error, and ollama.service is
      # ordered before this unit), but it does mean an ollama outage is now a
      # memory outage. `provider = "none"` is the deliberate FTS-only setting if
      # that ever needs backing out.
      #
      # Changing provider, model, chunking or scope invalidates the SQLite
      # vector index. openclaw does not silently re-embed -- it pauses vector
      # search and reports an index identity warning until
      # `openclaw memory index --force` is run.
      memorySearch = {
        provider = "ollama";
        model = "nomic-embed-text";
      };
    };

    # Matrix is how this thing reaches me: DMs in, notifications and cron job
    # output back. Everything below is either a credential, a gate on who may
    # drive the agent, or a note about why the obvious alternative is wrong.
    #
    # The plugin does NOT have to be installed first, despite what
    # docs/channels/matrix.md says ("openclaw plugins install @openclaw/matrix"
    # -- a command Nix mode refuses). 2026.7.x ships it in-tree at
    # extensions/matrix and ../../../pkgs/openclaw.nix copies both the source
    # and dist/extensions/matrix into the output, which makes its origin
    # "bundled". canStartConfiguredChannelPlugin returns true unconditionally
    # for bundled plugins, so a channels.matrix block is the whole activation
    # story. The manifest's `activation.onStartup: false` governs lazy loading
    # in the CLI, not the gateway.
    #
    # One caveat carried by that same packaging: see the matrixCryptoNative
    # note in pkgs/openclaw.nix. Encrypted *text* rides the wasm crypto module
    # and always worked; encrypted *media* needs a native binding that npm only
    # ships via a postinstall downloader, which is why the package vendors it.
    channels.matrix = {
      enabled = true;
      homeserver = "https://${matrixServerName}";

      # Without this the channel never starts. It dies immediately with
      #   channel exited: Blocked: resolves to private/internal/special-use IP
      #   address
      # then auto-restarts on a doubling backoff and gives up after 10 tries,
      # leaving the gateway healthy and the channel silently dead.
      #
      # The guard is an SSRF protection: openclaw resolves the homeserver name
      # and refuses private ranges unless the account opts in. bot.nel.family is
      # a *public* name, but romeo's own unbound answers it with 192.168.3.7 --
      # the split-horizon hairpin from ../unbound.nix -- so the guard sees a
      # LAN address and blocks. Nothing about the name being public helps; the
      # check is on the resolved address.
      #
      # "dangerously" is aimed at the case where a prompt-injected agent picks
      # the homeserver, which cannot happen here: the value is pinned in this
      # file and Nix mode forbids the gateway rewriting it. Upstream documents
      # exactly this setting for LAN/Tailscale/internal homeservers
      # (docs/channels/matrix.md, "Private/LAN homeservers").
      #
      # Same class of exemption as models.providers.cliproxy's
      # request.allowPrivateNetwork above, for the same reason: the service
      # being talked to is on this host.
      network.dangerouslyAllowPrivateNetwork = true;

      # Password auth rather than a pasted access token, which is the whole
      # reason for self-hosting: ./matrix.nix creates this account *from* this
      # password, so declaring the account and configuring the client are the
      # same act. A token would have to be minted by hand from a running
      # server and pasted back into agenix -- a manual step per agent, and one
      # that cannot be reproduced from the repo alone.
      #
      # The plugin logs in once and caches the resulting token under
      # <state>/credentials/matrix/, so the password is not sent on every
      # start. userId is mandatory on this path (there is no token to run
      # /whoami against).
      #
      # Substituted from the process environment at config load, exactly like
      # gateway.auth.token above -- resolveConfigEnvVars walks the whole parsed
      # config, not a fixed list of keys, and throws MissingEnvVarError naming
      # this path if the var is unset or empty.
      userId = "@openclaw:${matrixServerName}";
      password = "\${MATRIX_PASSWORD}";

      # Names the session in your Matrix client's device list, which is what
      # you cross-sign during verification. Worth being specific: an unnamed
      # device is indistinguishable from a stolen-token session.
      deviceName = "OpenClaw (romeo)";

      # Rust crypto via matrix-js-sdk. The device starts unverified, which
      # makes clients render shields on its messages until it is cross-signed;
      # `openclaw-admin matrix encryption setup` (below) does that bootstrap.
      #
      # startupVerification is left at its default of "if-unverified", so an
      # unverified gateway asks this account to verify it at startup, at most
      # once per 24h. That is the intended first-run prompt, not a bug.
      encryption = true;

      # Who may DM the agent. "pairing" (the upstream default flow) would let
      # any stranger start a pairing handshake; "allowlist" answers exactly one
      # MXID and ignores the rest. This is the whole authorization story for an
      # agent with shell access on romeo, so it stays closed.
      dm = {
        policy = "allowlist";
        allowFrom = ownerMxids;
      };

      # Rooms are a different surface with different failure modes (other
      # members, invite-driven membership, mention parsing), and nothing here
      # needs them yet. Disabled means group messages are dropped even in rooms
      # the bot has joined.
      groupPolicy = "disabled";

      # autoJoin has to be permissive for the *first* DM to exist at all:
      # invites arrive before openclaw can classify them as DM or group, so
      # autoJoin runs first and dm.policy only applies afterwards, and
      # autoJoinAllowlist accepts room ids/aliases -- neither of which is known
      # before the room does. "off" would mean the bot never joins anything and
      # the channel is inert.
      #
      # Joining is not answering: with the two policies above, an uninvited
      # room is joined and then completely ignored. Once the DM room exists,
      # tighten this to autoJoin = "allowlist" with its concrete
      # "!roomid:${matrixServerName}" (get it from your client's room settings,
      # or `openclaw-admin channels resolve --channel matrix`).
      autoJoin = "always";
    };

    # Belt and braces. Bundled channel plugins start on the strength of the
    # channels.matrix block alone, but Nix mode means the config file is the
    # only surface that can ever enable a plugin -- `openclaw plugins enable`
    # is refused -- so the intent is stated where it can actually be acted on.
    plugins.entries.matrix.enabled = true;

    # Dreaming: nightly background consolidation of the agent's own memory.
    #
    # What it actually does, because the name oversells it: one cron sweep runs
    # three phases (light -> REM -> deep) and only deep writes anything durable.
    # Deep ranks accumulated short-term recall signals on six weighted scores
    # (relevance 0.30, frequency 0.24, query diversity 0.15, recency 0.15,
    # consolidation 0.10, conceptual richness 0.06) and appends what clears
    # minScore 0.8 / minRecallCount 3 / minUniqueQueries 3 to MEMORY.md, capped
    # at 10 per sweep with a 14-day recency half-life and a 30-day max age.
    #
    # That ranking is plain deterministic TypeScript -- no model is consulted
    # about what deserves to be remembered. The ONLY model-driven part is the
    # Dream Diary, up to three short prose entries per sweep written into
    # DREAMS.md for human reading. Diary text is explicitly excluded from
    # promotion, so `model` below is a prose-quality knob and nothing more.
    #
    # Two things that would otherwise look like Nix-mode problems and are not:
    # the managed cron job lives in the state sqlite, not openclaw.json, so
    # memory-core can create it without touching the read-only config; and the
    # cron payload is an agentTurn carrying a sentinel string that memory-core
    # intercepts before it reaches a model, so the sweep itself costs no tokens.
    #
    # Worth watching: MEMORY.md is injected into every session bootstrap and
    # this agent has shell access on romeo, so an auto-promoted entry is durable
    # instruction context. channels.matrix's dm allowlist above is what bounds
    # who can seed the material. Review DREAMS.md and `openclaw memory promote`
    # (preview, no --apply) before trusting the pipeline unattended.
    plugins.entries.memory-core = {
      # memory-core is the default memory backend and would load anyway; stated
      # for the same reason the matrix entry above is -- in Nix mode this file
      # is the only surface that can enable a plugin at all.
      enabled = true;

      # The trust gate for `dreaming.model`. Plugin-initiated subagent runs may
      # not choose their own model unless an operator opts in, and the allowlist
      # narrows that opt-in to exactly the one model the diary is meant to use
      # rather than handing memory-core a blank cheque against cliproxy.
      subagent = {
        allowModelOverride = true;
        allowedModels = ["ollama/gemma4:12b"];
      };

      config.dreaming = {
        enabled = true;

        # Local, and that is the point. Unset, the diary inherits
        # agents.defaults.model.primary and ships memory fragments -- the most
        # personal text this host holds -- through cli-proxy-api to ChatGPT
        # every night, on subscription quota, to write poetry.
        #
        # gemma4:12b over the faster qwen3.5 line on measured instruction
        # following, not vibes. Against the real narrative prompt on this GPU,
        # qwen3.5:9b and :4b both opened with "The dream began/felt ..." -- the
        # prompt forbids exactly that meta-commentary -- while gemma4:12b held
        # every constraint including the 80-180 word range (139 words). Cost of
        # the swap is throughput: 32.5 tok/s vs 49.6 and 72.1, which does not
        # matter for 140 words at 03:00.
        #
        # Timing on romeo: 6.5s warm, 20-26s cold. Cold is the common case for
        # the first phase of a sweep and it competes with whatever else the B580
        # is doing, which is why ../../../pkgs/openclaw.nix raises the diary
        # subagent timeout from 60s to 5 minutes -- overrunning it is not an
        # error, it silently writes placeholder filler into DREAMS.md instead.
        #
        # 8.4 GiB resident of the B580's ~10.2 GiB, which is why ../ollama.nix
        # pairs it with the ~0.3 GiB nomic-embed-text rather than a larger
        # encoder: both stay loaded instead of evicting each other nightly.
        model = "ollama/gemma4:12b";

        # Default cadence, but the timezone is NOT default -- unset, the cron
        # expression is evaluated in UTC, so "0 3 * * *" would fire at 20:00
        # local and the "daily" lookback windows would straddle local days.
        frequency = "0 3 * * *";
        timezone = "America/Denver";
      };
    };

    # `/pair qr` has to embed a URL the phone can actually reach. The gateway
    # only knows it is bound to 127.0.0.1, so without this it refuses outright:
    #   Gateway is only bound to loopback. Set gateway.bind=lan, enable
    #   tailscale serve, or configure plugins.entries.device-pair.config.publicUrl
    #
    # publicUrl is the right one of those three for a reverse-proxied setup --
    # it tells the pairing plugin what the outside world calls this gateway
    # while leaving the socket on loopback. The alternatives are both wrong
    # here:
    #
    #   - gateway.bind = "lan" puts the raw gateway on the LAN, bypassing
    #     nginx's TLS and the tailnet/LAN allowlist that is the only thing
    #     restricting access to an agent with shell access.
    #   - `tailscale serve` would break unrelated services. Per ./goose.nix, it
    #     intercepts its configured ports inside tailscaled's netstack ahead of
    #     the host socket and hard-fails any SNI that is not the node's
    #     *.ts.net name -- so claiming 443 would take out every other vhost on
    #     romeo (jellyfin, immich, nextcloud, ...).
    #
    # Reachability of this name is a DNS question, not an nginx one: it resolves
    # to 192.168.3.7 on the LAN via unbound's h.b.nel.family redirect zone, but
    # a roaming client follows the *.h.b.nel.family wildcard to the public A
    # record and lands in `deny all`. Off-LAN pairing needs the explicit CNAME
    # to romeo.b.nel.family that ./goose.nix documents.
    #
    # No `enabled = true` needed: device-pair ships enabledByDefault and the
    # gateway loads it at startup. Running `openclaw qr` from a shell prints
    # "plugin disabled (bundled (disabled by default)) but config is present" --
    # that is the CLI's own activation context talking, not the gateway's, and
    # the QR renders anyway. publicUrl is the plugin's only accepted key
    # (configSchema sets additionalProperties: false).
    plugins.entries.device-pair.config.publicUrl = "https://${domain}";
  };
in {
  # Authenticates every Control UI / CLI client to the gateway. Generated rather
  # than hand-written so it never exists outside agenix.
  age.secrets.openclaw-gateway-token = {
    rekeyFile = ./secrets/openclaw_gateway_token.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -hex 32";
    # Nobody ever types this value, so without a Bitwarden copy the only ways to
    # read it are decrypting the repo secret or catting /run/agenix on romeo.
    #
    # `username` is what makes this a *login* item rather than a secure note,
    # and only login items carry URIs -- dropping it would silently cost the
    # domain match this exists for. There is no real username to record: the
    # Control UI prompts for a bearer token, so "token" names what the password
    # field actually holds.
    bitwarden = {
      name = "OpenClaw Gateway Token";
      username = "token";
      uris = {
        uri = "https://${domain}";
        matchType = "host";
      };
      notes = "Gateway auth token for OpenClaw on romeo-2 (gateway.auth.token). Paste it into the Control UI at https://${domain} on first connect from each browser; the device is remembered afterwards. Leave the password field empty -- no gateway password is configured. Rotating it means regenerating the agenix secret and rebuilding, since Nix mode forbids the gateway editing its own config.";
    };
  };

  # CLI_PROXY_API_KEY is the *proxy's* key, not a real OpenAI key -- it only
  # authenticates a localhost client to cli-proxy-api, which holds the actual
  # ChatGPT OAuth credential. Same secret librechat.nix and goose.nix present.
  age-template.files.openclaw-env = {
    vars = {
      GATEWAY_TOKEN = config.age.secrets.openclaw-gateway-token.path;
      PROXY_KEY = config.age.secrets.cli-proxy-api-key.path;
      # Declared by ./matrix.nix, not here: it is the same secret the
      # homeserver created the @openclaw account with.
      MATRIX_PW = config.age.secrets.matrix-password-openclaw.path;
    };
    owner = "openclaw";
    group = "openclaw";
    mode = "0400";
    content = ''
      OPENCLAW_GATEWAY_TOKEN=$GATEWAY_TOKEN
      CLI_PROXY_API_KEY=$PROXY_KEY
      MATRIX_PASSWORD=$MATRIX_PW
    '';
  };

  users.users.openclaw = {
    isSystemUser = true;
    group = "openclaw";
    home = stateDir;
  };
  users.groups.openclaw = {};

  # openclaw itself for the read-only, environment-independent subcommands
  # (`openclaw --version`, `--help`); openclaw-admin for anything that has to
  # see the daemon's config, state or credentials -- which is every `matrix`
  # and `doctor` invocation. See the note on openclawAdmin above.
  environment.systemPackages = [openclaw openclawAdmin];

  systemd.tmpfiles.rules = [
    "d ${stateDir} 0700 openclaw openclaw -"
    "d ${workspace} 0750 openclaw openclaw -"
  ];

  systemd.services.openclaw = {
    description = "OpenClaw gateway";
    wantedBy = ["multi-user.target"];
    # ollama.service is ordered but not required: the gateway starts fine
    # without it and simply discovers an empty ollama catalog, which would
    # leave `ollama/*` refs unresolvable until something re-triggers discovery.
    # Ordering after it makes the catalog populated on a clean boot.
    # continuwuity is ordered for the same reason ollama is, but the failure it
    # prevents is nastier: the @openclaw account does not exist until
    # ./matrix.nix's admin_execute runs at homeserver startup, and a gateway
    # that comes up first gets M_FORBIDDEN on login and leaves the channel dead
    # rather than retrying forever. Ordering is not a guarantee -- account
    # creation happens a moment *after* continuwuity is up -- so a first boot
    # can still need one `systemctl restart openclaw`.
    wants = ["network-online.target" "cli-proxy-api.service" "ollama.service" "continuwuity.service"];
    after = ["network-online.target" "cli-proxy-api.service" "ollama.service" "continuwuity.service"];

    environment = {
      OPENCLAW_NIX_MODE = "1";
      OPENCLAW_CONFIG_PATH = openclawConfig;
      OPENCLAW_STATE_DIR = stateDir;
      OPENCLAW_WORKSPACE_DIR = workspace;
      HOME = stateDir;

      # Required, and not obviously so: the `apiKey = "ollama-local"` in the
      # provider block above is enough for catalog *discovery*, but the agent
      # resolves the credential for an actual turn out of its own auth store
      # and falls back to this env var, not to provider config. Without it the
      # models list fine and every turn dies with
      #   No API key found for provider "ollama". Auth store:
      #   /var/lib/openclaw/agents/main/agent/openclaw-agent.sqlite
      # Not a secret -- see the marker note on models.providers.ollama.apiKey.
      #
      # Side effect worth knowing: the bundled ollama plugin declares this same
      # env var for its `ollama-cloud` provider, so setting it also makes four
      # hosted models (kimi-k2.5, minimax-m2.7, glm-5.1, glm-5.2) appear in the
      # catalog as authenticated. They are not: ollama.com rejects the local
      # marker, so selecting one fails the turn rather than silently shipping a
      # prompt off-box. Cosmetic noise in the picker, nothing more.
      OLLAMA_API_KEY = "ollama-local";
    };

    # The agent shells out; give it the same modest toolset goose.nix does
    # rather than leaving it with a bare PATH. Shared with openclaw-admin so a
    # command run by hand sees the same tools the daemon would.
    path = agentTools;

    serviceConfig = {
      ExecStart = "${lib.getExe openclaw} gateway --port ${toString port}";
      User = "openclaw";
      Group = "openclaw";
      StateDirectory = "openclaw";
      StateDirectoryMode = "0700";
      WorkingDirectory = workspace;
      EnvironmentFile = config.age-template.files.openclaw-env.path;
      Restart = "on-failure";
      RestartSec = "10s";

      # Deliberately moderate hardening, matching goose.nix: this thing exists
      # to spawn shell commands and plugin subprocesses, so the aggressive
      # filters used on cli-proxy-api (MemoryDenyWriteExecute, ~@privileged)
      # would break it. What is kept still blocks privilege escalation and
      # confines writes to the state dir and PrivateTmp.
      #
      # This sandbox is the only thing bounding a prompt-injected agent -- see
      # the knownVulnerabilities note above. ProtectSystem only blocks *writes*,
      # so the agent can still read /mnt/vault and friends and relay what it
      # reads to the model API. goose.nix documents why InaccessiblePaths= was
      # rejected there (it masks mounts with tmpfs and corrupts `df` output);
      # the same tradeoff applies here.
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
      RestrictAddressFamilies = ["AF_INET" "AF_INET6" "AF_UNIX"];
    };
  };

  services.nginx.virtualHosts."${domain}" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      client_max_body_size 0;
      #Allow access from Tailscale network
      allow 100.64.0.0/10;
      allow fd7a:115c:a1e0::/48;
      #Allow access from local network
      allow 192.168.0.0/16;
      deny all;
    '';
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString port}";
      # The Control UI drives the gateway over a long-lived WebSocket and an
      # agent turn can sit quiet for minutes while a tool runs, so the default
      # 60s read timeout would drop it mid-task.
      proxyWebsockets = true;
      extraConfig = ''
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
        proxy_buffering off;
      '';
    };
  };
}
