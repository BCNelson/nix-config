{ pkgs, ... }:
let
  sensitive = import ../../../sensitive.nix;
in
{
  users.users.bcnelson = {
    # TODO: make this more generic
    isNormalUser = true;
    description = "Bradley Nelson";
    extraGroups = [ "networkmanager" "wheel" "plugdev" "docker" "dialout" ];
    # The install script will change mark this user as needing a password change on first login.
    # Note SDDM does not support password changes so this will need to be done via the command line.
    initialPassword = sensitive.bcnelson_init_password;
    packages = with pkgs; [
      # Console Apps here
    ];
  };
}
