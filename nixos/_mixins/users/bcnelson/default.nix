{ }
{
  users.users.bcnelson = {
    # TODO: make this more generic
    isNormalUser = true;
    description = "Bradley Nelson";
    extraGroups = [ "networkmanager" "wheel" "plugdev" "docker" ];
    packages = with pkgs; [
      # Console Apps here
    ];
  };
}
