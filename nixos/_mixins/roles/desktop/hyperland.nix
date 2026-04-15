{ pkgs, ... }:
{
  programs.hyprland = {
    enable = true;
    xwayland.enable = true;
  };

  environment.systemPackages = with pkgs; [
    waybar
    dunst
    libnotify
    rofi-wayland
    hyprpaper
    grim
    slurp
    wl-clipboard
    nautilus
    polkit_gnome
  ];

  xdg.portal = {
    enable = true;
    extraPortals = [ pkgs.xdg-desktop-portal-hyprland ];
  };

  security.pam.services.hyprlock = {};
}
