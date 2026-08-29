{pkgs, ...}: {
  services.ollama = {
    enable = true;
    # The nginx vhost at the bottom of this file is the only remote entry
    # point; do not expose Ollama on the LAN, where its unauthenticated API
    # would otherwise be reachable.
    host = "127.0.0.1";
    port = 11434;
    loadModels = [
      "qwen3:4b"
      "qwen3.5:0.8b"
      "qwen3.5:2b"
      "qwen3.5:4b"
      "qwen3.5:9b"
      "gemma4:12b"
      "gemma4:12b-it-qat"
      "deepseek-r1:1.5b"
      "deepseek-r1:7b"
      "deepseek-r1:8b"
      "deepseek-coder:6.7b"

      # Embeddings, not chat. ../services/openclaw.nix points
      # agents.defaults.memorySearch at this model so memory_search runs hybrid
      # vector+BM25 recall instead of the keyword-only FTS fallback it gets with
      # no embedding provider configured.
      #
      # nomic-embed-text rather than a stronger encoder because it is openclaw's
      # own default for the ollama adapter (DEFAULT_OLLAMA_EMBEDDING_MODEL) and
      # one of only three prefixes its embedding provider carries an asymmetric
      # query template for -- it prepends "search_query: " to searches, which
      # this model was trained to expect and which a generic endpoint would not
      # do. The alternatives with templates are mxbai-embed-large (335M) and
      # qwen3-embedding:0.6b; both are better encoders and both cost more of the
      # B580's ~10.2 GiB, which gemma4:12b already occupies 8.4 GiB of. At ~0.3
      # GiB this one coexists with a resident chat model instead of evicting it
      # on every recall.
      #
      # NOT declared in openclaw's models.providers.ollama catalog: that list is
      # the *chat* catalog and an entry there would put an embedding model in
      # the agent's model picker. The embedding adapter reads the model name
      # from memorySearch.model and only borrows the provider's baseUrl and key.
      #
      # Vulkan: verified clean on the B580, which was not a given. The
      # corruption GGML_VK_DISABLE_ASYNC below works around was found by reading
      # doubled words in generated text; a corrupted embedding has no such tell,
      # it just retrieves badly. So it was measured instead, against a
      # throwaway OLLAMA_VULKAN=0 server on 11435 sharing this model store:
      #
      #   B580 vs CPU, same text          cos 0.999996-0.999998 (F16 rounding)
      #   B580 twice, same batch          cos 1.0 exactly (deterministic)
      #   B580 batched vs one-at-a-time   cos 1.0 exactly
      #   retrieval ranking B580 vs CPU   identical order, scores within 3e-4
      #
      # And DISABLE_ASYNC turns out NOT to be load-bearing here: a B580 server
      # run with async submission left on matched CPU to the same 0.999996+.
      # Consistent with the failure mode -- embedding is one forward pass with
      # no iterative decode, so there is no token stream to interleave. Keep the
      # workaround for generation; do not expect to need it for the index.
      #
      # Re-measure on ollama/mesa bumps the same way. Note that comparing two
      # embeddings needs the index bound to a jq variable (`. as $i`) before
      # indexing inside a cosine helper -- jq function args are closures
      # evaluated against the callee's input, and getting that wrong silently
      # returns a constant for every pair, which reads as a perfect match.
      "nomic-embed-text"
    ];
    package = pkgs.ollama-vulkan;
    environmentVariables = {
      # Enable Vulkan backend
      OLLAMA_VULKAN = "1";
      # Target B580 (xe driver, device 1) - A380 (i915) loads first as device 0
      GGML_VK_VISIBLE_DEVICES = "1";
      # Maximize GPU layer usage
      OLLAMA_GPU_LAYERS = "-1";
      # Note: GGML_VK_DISABLE_INTEGER_DOT_PRODUCT removed - that's for iGPUs with driver bugs,
      # the B580 discrete GPU should support integer dot products correctly

      # REQUIRED FOR CORRECT OUTPUT on this GPU -- not a tuning knob.
      #
      # Without it every model on the B580 emits two token streams interleaved
      # into one and the daemon is useless: "Say hello." comes back as
      # "Hello! there How can I be help of you? today", and longer generations
      # degrade into doubled words ("OkayOkay, the user asked for
      # 'twotwo sentences'"). Setting it, the same prompts answer cleanly.
      #
      # Bisected on ollama 0.32.7 with throwaway servers on port 11435 sharing
      # this model store, one at a time (a stale server holding the port will
      # silently answer for every variant and fake a result -- assert the port
      # is free between runs). Corruption tracks the device, not the model:
      #   CPU (OLLAMA_VULKAN=0)              -> clean
      #   A380 / i915 (VISIBLE_DEVICES=0)    -> clean, 100% GPU
      #   B580 / xe   (VISIBLE_DEVICES=1)    -> corrupt
      # and on the B580 every other ggml-vulkan escape hatch was ruled out:
      # flash attention off, DISABLE_{FUSION,GRAPH_OPTIMIZE,COOPMAT,COOPMAT2,
      # INTEGER_DOT_PRODUCT,DOT2,F16,MMVQ,BFLOAT16,MULTI_ADD} all stayed
      # corrupt. Only DISABLE_ASYNC fixed it: 3/3 corrupt without, 5/5 clean
      # with, confirmed again per-model on qwen3.5:9b and qwen3:4b.
      #
      # That the fix is the *async submission* path matches the symptom -- two
      # interleaved streams is what unsynchronised command-buffer submission
      # looks like, not a bad matmul kernel. Upstream has a pile of open
      # "Intel Arc + Vulkan produces garbled output" reports
      # (ggml-org/llama.cpp#20097 blames a b8184 regression on a 3x A770 box,
      # #24560 is a B580, ollama#13964/#15248 are Arrow Lake) but none of them
      # names this env var, so do not expect to find this workaround written
      # down anywhere else.
      #
      # Costs ~4-6% tokens/sec, measured: qwen3.5:9b 53.2 -> 49.9 tok/s,
      # qwen3:4b 96.8 -> 93.2. Cheap next to the alternatives, which were
      # giving up the B580's 12G for the A380 that ../gamestream.nix reserves
      # for its compositor and VA-API encode, or falling back to CPU.
      #
      # Re-test on ollama/mesa bumps: drop this, ask a model to say hello, and
      # look for doubled words. Currently mesa 26.2.0, ollama 0.32.7.
      GGML_VK_DISABLE_ASYNC = "1";

      # Default is 5 minutes, which is too short now that ../services/openclaw.nix
      # puts a local model on a *blocking* path. Its active-memory plugin runs a
      # recall sub-agent in front of every DM reply on gemma4:12b. Measured here
      # via /api/generate, forcing eviction with keep_alive=0 between runs:
      # 16.9s cold (14.8s of that model load) against 1.9s warm. At the
      # 5-minute default any conversation with a pause longer than a coffee
      # refill pays that 14.8s again on its next message, so the slow path would
      # be the common one.
      #
      # 60m rather than -1 (never unload). The B580's ~10.2 GiB fits gemma4:12b
      # at 8.4 plus the 0.3 GiB nomic-embed-text encoder, and that is close
      # enough to the ceiling that pinning weights forever would make picking any
      # other model from the catalog an eviction. An hour covers a working day of
      # intermittent DMs and the 03:00 dreaming sweep's three phases without
      # holding the GPU hostage overnight.
      #
      # This is a residency knob, not a correctness one -- shortening it costs
      # latency, never answers. If cold loads still show up in active-memory
      # logs, -1 is the next step; if something else needs the B580, drop it.
      OLLAMA_KEEP_ALIVE = "60m";
    };
  };

  # Ensure ollama user has GPU access
  systemd.services.ollama.serviceConfig = {
    SupplementaryGroups = ["render" "video"];
  };

  # No `tailscale serve` here on purpose. It used to publish 11434 on the
  # tailnet, but `tailscale serve <port>` defaults to HTTPS 443, and Serve
  # intercepts its ports inside tailscaled's netstack ahead of the host socket
  # -- so it answered every SNI on romeo's tailnet address with the node's
  # *.ts.net cert and made nginx unreachable there for every vhost.
  # ../services/goose.nix needs 443 to fall through to nginx.
  #
  # Nothing was lost. Every on-host consumer talks to ollama over loopback
  # (librechat.nix, tendant.nix, goose.nix all use http://127.0.0.1:11434), and
  # the tailnet endpoint never worked anyway: ollama rejects any request whose
  # Host is not localhost, so it returned 403 to everything Serve forwarded.
  # The vhost below is the remote entry point that comment used to point at.

  # --- Remote access for Home Assistant (LAN + tailnet only) ---
  # Home Assistant runs on its own box (192.168.3.6, HA OS -- not in this repo)
  # and its Ollama integration is the one consumer that is not on romeo, so it
  # cannot use the loopback socket every other consumer uses. Front ollama with
  # the same LAN/tailnet-only vhost shape as ../services/librechat.nix rather
  # than moving services.ollama.host off 127.0.0.1: binding the daemon to the
  # LAN would publish an unauthenticated API -- including /api/delete and
  # /api/create -- to every device on the network, where this publishes a
  # single allowlisted port and keeps the daemon itself on loopback.
  #
  # *.h.b.nel.family already resolves to romeo (192.168.3.7) for LAN clients:
  # ../unbound.nix has a "h.b.nel.family" redirect local-zone, so this name
  # needs no new DNS record. The cert is DNS-01 via porkbun like every other
  # vhost, so it is trusted by HA without an exception.
  services.nginx.virtualHosts."ollama.h.b.nel.family" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      # Allow access from Tailscale network
      allow 100.64.0.0/10;
      # Allow access from local network
      allow 192.168.0.0/16;
      deny all;

      # Model uploads are not expected here, but /api/create with a blob is
      # large enough that the default 1m would truncate it.
      client_max_body_size 0;
    '';
    locations."/" = {
      proxyPass = "http://127.0.0.1:11434";

      # Off deliberately. The module emits the recommended-proxy include
      # *after* this location's extraConfig, and nginx takes the last
      # proxy_set_header for a header -- so with it on, the include's
      # `Host $host` would win over the rewrite below and ollama would 403
      # every request. See [[nginx-recommended-proxy-xff]] for the same
      # ordering trap on X-Forwarded-For.
      recommendedProxySettings = false;

      extraConfig = ''
        # ollama's allowed-hosts middleware only accepts localhost/127.0.0.1
        # while the daemon is bound to loopback; the real Host would be
        # ollama.h.b.nel.family and come back 403. No port: ollama falls back
        # to comparing the whole Host string when it does not parse as
        # host:port, and the bare address passes both paths.
        proxy_set_header Host 127.0.0.1;

        # Same middleware rejects a cross-origin browser request. HA sends no
        # Origin, but blank it so a browser pointed at this name cannot drive
        # the API either.
        proxy_set_header Origin "";

        # Declaring any proxy_set_header here resets the inherited set from the
        # http-level recommended block (X-Real-IP, X-Forwarded-*). That is fine
        # -- ollama reads none of them -- but it is why they are not restated.

        proxy_http_version 1.1;

        # The http-level default is 60s, which a 12b model would blow through:
        # a cold load is ~15s before the first token and a long generation runs
        # well past a minute. Streaming responses need buffering off, or nginx
        # holds tokens until the whole reply is done.
        proxy_read_timeout 600s;
        proxy_send_timeout 600s;
        proxy_buffering off;
      '';
    };
  };
}
