{ pkgs, libx, ... }:
let
  testDocker = libx.createDockerComposeStackPackage {
    name = "test-docker";
    src = ./config/test;
    dockerComposeDefinition = {
      version = "3.8";
      services = {
        hello_world = {
          image = "hello-world";
        };
      };
    };
  };
in
{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/zfs.nix
    ];

  environment.systemPackages = [
    pkgs.zfs
    testDocker
  ];
}
