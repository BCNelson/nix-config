{ config, ... }:
{
  imports = [
    ../_mixins/roles/tailscale.nix
  ];

  age.secrets.cadence_check_auto_update_berg.rekeyFile =
    ../../secrets/store/cadence/checks/auto-update-berg.age;

  # Automatic updates
  services.bcnelson.autoUpdate = {
    enable = true;
    path = "/config";
    reboot = true;  # Ensure updates are fully applied
    refreshInterval = "24h";  # Daily updates
    healthCheck = {
      enable = true;
      url = "https://health.b.nel.family";
      uuidFile = config.age.secrets.cadence_check_auto_update_berg.path;
    };
  };

  # Hide my user from the login screen it will still be accessible via the other users section
  services.displayManager.sddm.settings = {
    Users = {
      HideUsers = "bcnelson";
    };
  };

  time.timeZone = "America/Los_Angeles";

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