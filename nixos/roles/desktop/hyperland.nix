{ pkgs, ... }:
{
    programs.hyprland = {
        enable = true;
        xwayland.enable = true;
    };

    environment.systemPackages = [
        # waybar
        (pkgs.waybar.overrideAttrs (oldAttrs: {
                mesonFlags = oldAttrs.mesonFlags ++ [ "-Dexperimental=true" ];
            })
        )
        pkgs.dunst
        pkgs.libnotify

        pkgs.hyprpaper
        pkgs.rofi-wayland
    ];

    xdg.portal.enable = true;
    xdg.portal.extraPortals = [ pkgs.xdg-desktop-portal-hyprland ];
}