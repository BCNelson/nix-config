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
      # Use first GPU (stable for single GPU systems)
      GGML_VK_VISIBLE_DEVICES = "0";
      # Maximize GPU layer usage
      OLLAMA_GPU_LAYERS = "-1";
    };
  };

  # Ensure ollama user has GPU access
  systemd.services.ollama.serviceConfig = {
    SupplementaryGroups = [ "render" "video" ];
  };
}
