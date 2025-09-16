{ pkgs, ... }:

{
  environment.systemPackages = [
    pkgs.fprintd
  ];
  services.fprintd = {
    enable = true;
  };
  
  # Disable fingerprint authentication for SDDM to prevent login delay
  security.pam.services.sddm.fprintAuth = false;
}
