{ pkgs, ... }:
{
  programs.appimage = {
    enable = true;
    binfmt = true;
    # appimage-run's FHS env (appimageTools.defaultFhsEnvArgs) has GTK3 but
    # not libepoxy, which GTK/Flutter AppImages (e.g. Protoflow) link against
    # and expect the host to provide.
    package = pkgs.appimage-run.override {
      extraPkgs = p: [ p.libepoxy ];
    };
  };
}
