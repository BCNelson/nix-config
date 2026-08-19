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
