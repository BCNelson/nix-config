{ pkgs, outputs, ... }:

{
  imports = [
    outputs.homeModules.autostart
    ../_mixins/programs/firefox.nix
    ./_mixins/kde.nix
  ];

  home.packages = [
    pkgs.kdePackages.yakuake
    pkgs.libreoffice-qt6-still
    pkgs.hunspell
    pkgs.hunspellDicts.en_US-large
  ];

  programs.bash.enable = true;

  services.freedesktop.autostart = {
    enable = true;
    packageSourced = [
      {
        package = pkgs.kdePackages.yakuake;
        path = "share/applications/org.kde.yakuake.desktop";
      }
    ];
  };

  services.kdeconnect = {
    enable = true;
    indicator = true;
  };

  systemd.user.startServices = "sd-switch";
}
