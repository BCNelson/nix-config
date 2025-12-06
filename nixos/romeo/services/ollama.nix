{ config, ... }:
{
  services.ollama = {
    enable = true;
    host = "0.0.0.0";
    port = 11434;
    # Models to automatically download on first startup
    loadModels = [ "qwen3:4b" ];
    acceleration = "vulkan";
    environmentVariables = {
      # Enable Vulkan backend for Intel Arc GPU
      OLLAMA_VULKAN = "1";
      # Target B580 (xe driver, device 1) - A380 (i915) loads first as device 0
      GGML_VK_VISIBLE_DEVICES = "1";
      # Maximize GPU layer usage
      OLLAMA_GPU_LAYERS = "-1";

      # https://github.com/ollama/ollama/issues/13086
      GGML_VK_DISABLE_INTEGER_DOT_PRODUCT = "1";
    };
  };

  # Ensure ollama user has GPU access
  systemd.services.ollama.serviceConfig = {
    SupplementaryGroups = [ "render" "video" ];
  };
}
