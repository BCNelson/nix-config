{ pkgs, ... }:
{
  fonts = {
    enableDefaultPackages = false;
    packages = [
      pkgs.noto-fonts
      pkgs.noto-fonts-cjk-sans
      pkgs.noto-fonts-emoji
      pkgs.ubuntu_font_family
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
