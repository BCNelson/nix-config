{
  config,
  pkgs,
  lib,
  desktop,
  ...
}:

{
  # Home Manager needs a bit of information about you and the paths it should
  # manage

  imports = [
    ./_mixins/firefox.nix
    ./_mixins/ghostty.nix
    ./_mixins/zen.nix
    ../_mixins/programs/chrome.nix
    ../_mixins/programs/vscode.nix
    ../_mixins/programs/zed.nix
    ../_mixins/programs/super-productivity.nix
  ]
  ++ lib.optional (
    builtins.isString desktop && builtins.pathExists ./_mixins/${desktop}.nix
  ) ./_mixins/${desktop}.nix;

  home.packages = [
    # Ghostty is the terminal (see ./_mixins/ghostty.nix); konsole stays
    # installed as the fallback for when it misbehaves -- it is also the only
    # one of the two that works in an X11 session.
    (config.lib.nixGL.wrap pkgs.kdePackages.konsole)

    # pkgs.quickemu
    # pkgs.quickgui

    (config.lib.nixGL.wrap pkgs.easyeffects)

    (config.lib.nixGL.wrap pkgs.unstable.obsidian)

    (config.lib.nixGL.wrap pkgs.kdePackages.filelight)

    (config.lib.nixGL.wrap pkgs.kdePackages.kate)

    # Chat
    (config.lib.nixGL.wrap pkgs.unstable.discord)

    (config.lib.nixGL.wrap pkgs.newsflash)

    #Dignostic tools
    pkgs.vulkan-tools
    pkgs.libva-utils
  ];

  programs.bash.enable = true;

  services.kdeconnect = {
    enable = true;
    indicator = true;
  };

  # Workaround for Failed to start unit kdeconnect-indicator.service: Unit tray.target not found.
  # - https://github.com/nix-community/home-manager/issues/2064
  systemd.user.targets.tray = {
    Unit = {
      Description = "Home Manager System Tray";
      Requires = [ "graphical-session-pre.target" ];
    };
  };

  services.syncthing.enable = true;

  systemd.user.startServices = "sd-switch";

  home.sessionVariables = {
    VISUAL = "kwrite";
  };
}
