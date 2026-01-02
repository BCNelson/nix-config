_:
{
  users.users.ldporter = {
    isNormalUser = true;
    description = "Lyndon Porter";
    extraGroups = [ "networkmanager" "plugdev" "dialout" ];
    initialPassword = "changeme";
    packages = [];
  };
}
