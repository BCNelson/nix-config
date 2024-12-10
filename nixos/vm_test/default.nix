{ lib, ... }:

{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
    ];

  services.displayManager.sddm.settings = {
    Users = {
      HideUsers = "bcnelson";
    };
  };

  users.users.bcnelson.initialPassword = lib.mkForce "password";
  users.users.dsross.initialPassword = lib.mkForce "password";
}
