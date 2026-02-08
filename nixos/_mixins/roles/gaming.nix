{ pkgs, ... }:
{
  nixpkgs.config.packageOverrides = pkgs: {
    steam = pkgs.steam.override {
      extraPkgs = pkgs: with pkgs; [
        xorg.libXcursor
        xorg.libXi
        xorg.libXinerama
        xorg.libXScrnSaver
        libpng
        libpulseaudio
        libvorbis
        stdenv.cc.cc.lib
        libkrb5
        keyutils
      ];
    };
  };
  programs.steam.enable = true;
  programs.gamemode.enable = true;
  environment.systemPackages = [
    pkgs.gamescope
    pkgs.steamtinkerlaunch
    pkgs.opentrack
  ];

  services.hardware.openrgb.enable = true;
}
