{ pkgs, ... }:

{
  environment.systemPackages = [
    pkgs.fprintd
  ];
  services.fprintd = {
    enable = true;
  };
}
