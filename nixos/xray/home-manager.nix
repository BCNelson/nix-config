{ inputs, outputs, ... }:
let
    stateVersion = "23.05";
    libx = import ../../../lib { inherit inputs outputs stateVersion; };
in
{
    #  home-manager.users.bcnelson = libx.mkHome { hostname = "xray-2"; username = "bcnelson"; desktop = "kde"; home-manager = inputs.home-manager-unstable; pkgs = inputs.nixpkgs-unstable; };
}