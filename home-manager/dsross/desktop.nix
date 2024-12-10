{ pkgs, outputs, ... }:

{
  # Home Manager needs a bit of information about you and the paths it should
  # manage

  imports = [
    outputs.homeManagerModules.autostart
    ../_mixins/programs/firefox.nix
    ./_mixins/kde.nix
  ];

  home.packages = [
    pkgs.yakuake
    pkgs.libreoffice-qt6-still
    pkgs.hunspell
    pkgs.hunspellDicts.en_US-large
  ];

  programs.bash.enable = true;

  services.freedesktop.autostart = {
    enable = true;
    packageSourced = [
      {
        package = pkgs.yakuake;
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
