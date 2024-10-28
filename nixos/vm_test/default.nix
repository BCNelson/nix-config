{ lib, ... }:

{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
    ];

  users.users.bcnelson.initialPassword = lib.mkForce "password";
}
