{ config, desktop, lib, pkgs, ... }:
let
  ifExists = groups: builtins.filter (group: builtins.hasAttr group config.users.groups) groups;
in
{
  # Only include desktop components if one is supplied.
  imports = lib.optional (builtins.isString desktop) ./desktop.nix;

  config.users.users.nixos = {
    description = "NixOS";
    extraGroups = [
      "audio"
      "networkmanager"
      "users"
      "video"
      "wheel"
    ]
    ++ ifExists [
      "docker"
      "podman"
    ];
    homeMode = "0755";
    packages = [ pkgs.home-manager pkgs.kdePackages.kate ];
  };

  config.system.stateVersion = lib.mkForce lib.trivial.release;
  config.environment.systemPackages = [ pkgs.install-system ];
  config.services.kmscon.autologinUser = "nixos";
}
