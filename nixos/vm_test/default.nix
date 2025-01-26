{ lib, pkgs, ... }:

{
  imports =
    [
      ./hardware-configuration.nix
    ];

  services.displayManager.sddm.settings = {
    Users = {
      HideUsers = "bcnelson";
    };
  };

  users.users.bcnelson.initialPassword = lib.mkForce "password";
  users.users.brnelson.initialPassword = lib.mkForce "password";

  environment.plasma6.excludePackages = with pkgs.kdePackages; [ 
    plasma-browser-integration
    konsole
    (lib.getBin qttools) # Expose qdbus in PATH
    ark
    elisa
    gwenview
    okular
    kate
    khelpcenter
    dolphin
    baloo-widgets # baloo information in Dolphin
    dolphin-plugins
    spectacle
    ffmpegthumbs
    krdp
  ];

  services.xserver.excludePackages = [ pkgs.xterm ];
}
