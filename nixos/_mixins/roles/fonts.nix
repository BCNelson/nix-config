{ pkgs, ... }:
{
  fonts = {
    enableDefaultPackages = false;
    packages = [
      pkgs.noto-fonts
      pkgs.noto-fonts-cjk-sans
      pkgs.noto-fonts-color-emoji
      pkgs.ubuntu-classic
      pkgs.monaspace
    ];
    fontconfig = {
      defaultFonts = {
        serif = [ "Ubuntu Serif" ];
        sansSerif = [ "Ubuntu" ];
        monospace = [ "Monaspace Neon" ];
      };
    };
  };
}
