{ lib, ... }:
{
  imports = [
    ../_mixins/work/guidecx/k2.nix
  ];
  programs = {
    git = {
      userEmail = lib.mkForce "bnelson@guidecx.com";
    };
  };
}
