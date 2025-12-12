{ pkgs, ... }:
{
  services.ollama = {
    enable = true;
    host = "0.0.0.0";
    port = 11434;
    loadModels = [ "qwen3:4b" ];
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
}
