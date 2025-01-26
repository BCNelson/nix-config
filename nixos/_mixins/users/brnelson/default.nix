{ libx, ... }:
let
  init_password = libx.getSecret ../../../sensitive.nix "hlnelson_init_password";
in
{

  users.users.brnelson = {
    # TODO: make this more generic
    isNormalUser = true;
    description = "Brynlee Nelson";
    extraGroups = [ "networkmanager" "plugdev" "dialout" ];
    # The install script will change mark this user as needing a password change on first login.
    # Note SDDM does not support password changes so this will need to be done via the command line.
    initialPassword = init_password;
    packages = [
      # Console Apps here
    ];
  };
}
