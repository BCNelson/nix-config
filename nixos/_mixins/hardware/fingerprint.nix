{ pkgs, ... }:

{
  environment.systemPackages = [
    pkgs.fprintd
  ];
  services.fprintd = {
    enable = true;
    tod = {
      enable = true;
      driver = pkgs.libfprint-2-tod1-goodix;
    };
  };
}
