{ pkgs, ... }:
{
  services.ollama = {
    enable = true;
    host = "127.0.0.1";
    port = 11434;
    package = pkgs.ollama-vulkan;
    environmentVariables = {
      # Enable Vulkan backend (RADV on the AMD Radeon GPU)
      OLLAMA_VULKAN = "1";
      # Maximize GPU layer usage
      OLLAMA_GPU_LAYERS = "-1";
    };
  };

  # Ensure ollama user has GPU access
  systemd.services.ollama.serviceConfig = {
    SupplementaryGroups = [ "render" "video" ];
  };
}
