{ pkgs, ... }:
let
  bcnelson_init_password = "bcnelson_init_password";
in
{
  age.secrets.happy_ntfy_topic = {
    rekeyFile = ../../../../secrets/store/ntfy_topic.age;
    owner = "bcnelson";
    mode = "0400";
  };

  users.users.bcnelson = {
    # TODO: make this more generic
    isNormalUser = true;
    description = "Bradley Nelson";
    extraGroups = [ "networkmanager" "wheel" "plugdev" "docker" "dialout" ];
    # The install script will change mark this user as needing a password change on first login.
    # Note SDDM does not support password changes so this will need to be done via the command line.
    initialPassword = bcnelson_init_password;
    packages = with pkgs; [
      vim
    ];
  };
}
