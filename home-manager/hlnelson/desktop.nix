{ pkgs, outputs, ... }:

{
  # Home Manager needs a bit of information about you and the paths it should
  # manage

  imports = [
    outputs.homeModules.autostart
    ../_mixins/programs/firefox.nix
    ../_mixins/programs/chrome.nix
    ../_mixins/programs/vscode.nix
  ];

  home.packages = [
    pkgs.kdePackages.yakuake

    pkgs.unstable.obsidian

    pkgs.kdePackages.kate

    pkgs.unstable.signal-desktop
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

  services.syncthing.enable = true;

  systemd.user.startServices = "sd-switch";

  # Home Manager is pretty good at managing dotfiles. The primary way to manage
  # plain files is through 'home.file'.
  home.file = {
    ".local/share/konsole/Fish.profile".text = ''
      [General]
      Command=~/.nix-profile/bin/fish
      Name=Fish
      Parent=FALLBACK/

      [Scrolling]
      HistoryMode=2
    '';
  };

  home.sessionVariables = {
    VISUAL = "kwrite";
  };
}
