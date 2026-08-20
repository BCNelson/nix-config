{ pkgs, ... }:
{
  services.ollama = {
    enable = true;
    # Tailscale Serve is the only remote entry point; do not expose Ollama on
    # the LAN, where its unauthenticated API would otherwise be reachable.
    host = "127.0.0.1";
    port = 11434;
    loadModels = [ 
      "qwen3:4b"
      "qwen3.5:0.8b"
      "qwen3.5:2b"
      "qwen3.5:4b"
      "qwen3.5:9b"
      "deepseek-r1:1.5b"
      "deepseek-r1:7b"
      "deepseek-r1:8b"
      "deepseek-coder:6.7b"
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
    };
  };

  # Ensure ollama user has GPU access
  systemd.services.ollama.serviceConfig = {
    SupplementaryGroups = [ "render" "video" ];
  };

  # No `tailscale serve` here on purpose. It used to publish 11434 on the
  # tailnet, but `tailscale serve <port>` defaults to HTTPS 443, and Serve
  # intercepts its ports inside tailscaled's netstack ahead of the host socket
  # -- so it answered every SNI on romeo's tailnet address with the node's
  # *.ts.net cert and made nginx unreachable there for every vhost.
  # ../services/goose.nix needs 443 to fall through to nginx.
  #
  # Nothing was lost. Every consumer talks to ollama over loopback
  # (librechat.nix, tendant.nix, goose.nix all use http://127.0.0.1:11434), and
  # the tailnet endpoint never worked anyway: ollama rejects any request whose
  # Host is not localhost, so it returned 403 to everything Serve forwarded.
  # If remote access is ever wanted, give it an nginx vhost with the usual
  # allow 100.64.0.0/10 + proxy_set_header Host 127.0.0.1.
}
