{ lib, ... }:
{
  imports = [
    ../_mixins/roles/tailscale.nix
  ];

  # Automatic updates
  services.bcnelson.autoUpdate = {
    enable = true;
    path = "/config";
    reboot = true;  # Ensure updates are fully applied
    refreshInterval = "24h";  # Daily updates
  };

  # Hide my user from the login screen it will still be accessible via the other users section
  services.displayManager.sddm.settings = {
    Users = {
      HideUsers = "bcnelson";
    };
  };

  # Enable automatic garbage collection
  nix = {
    settings = {
      auto-optimise-store = true;
    };
    gc = {
      automatic = true;
      dates = "weekly";
      options = "--delete-older-than 30d";
    };
  };

  # Enable zram swap for better performance on limited RAM
  zramSwap.enable = true;
}