{ config, pkgs, inputs, ... }:

{
  # Home Manager needs a bit of information about you and the paths it should
  # manage.
  home.username = "bcnelson";
  home.homeDirectory = "/home/bcnelson";

  imports = [
    ./firefox.nix
    ./chrome.nix
    ./vscode.nix
  ];

  # This value determines the Home Manager release that your configuration is
  # compatible with. This helps avoid breakage when a new Home Manager release
  # introduces backwards incompatible changes.
  #
  # You should not change this value, even if you update Home Manager. If you do
  # want to update the value, then make sure to first check the Home Manager
  # release notes.
  home.stateVersion = "23.05"; # Please read the comment before changing.

  home.packages = [
    # # Adds the 'hello' command to your environment. It prints a friendly
    # # "Hello, world!" when run.
    # pkgs.hello
    pkgs.deckmaster
    pkgs.yakuake
    pkgs.git
    pkgs.obsidian
    pkgs.neochat

    # # It is sometimes useful to fine-tune packages, for example, by applying
    # # overrides. You can do that directly here, just don't forget the
    # # parentheses. Maybe you want to install Nerd Fonts with a limited number of
    # # fonts?
    # (pkgs.nerdfonts.override { fonts = [ "FantasqueSansMono" ]; })

    # # You can also create simple shell scripts directly inside your
    # # configuration. For example, this adds a command 'my-hello' to your
    # # environment:
    # (pkgs.writeShellScriptBin "my-hello" ''
    #   echo "Hello, ${config.home.username}!"
    # '')
  ];

  programs = {
    fish = {
      enable = true;
      interactiveShellInit = ''
        set fish_greeting # Disable greeting
      '';
      plugins = [
        {
          name = "done";
          src = pkgs.fishPlugins.done.src;
        }
        {
          name = "z";
          src = pkgs.fishPlugins.z.src;
        }
      ];
    };
    git = {
      enable = true;
      userName = "Bradley Nelson";
      userEmail = "bradely@nel.family";
    };
  };

  xdg.enable=true;
  xdg.mime.enable=true;
  targets.genericLinux.enable=true;
  xdg.systemDirs.data = [ "${config.home.homeDirectory}/.nix-profile/share/applications" ];
  programs.bash.enable = true;

  # Home Manager is pretty good at managing dotfiles. The primary way to manage
  # plain files is through 'home.file'.
  home.file = {
    # # Building this configuration will create a copy of 'dotfiles/screenrc' in
    # # the Nix store. Activating the configuration will then make '~/.screenrc' a
    # # symlink to the Nix store copy.
    # ".screenrc".source = dotfiles/screenrc;

    ".local/share/konsole/Fish.profile".text = ''
    [General]
    Command=~/.nix-profile/bin/fish
    Name=Fish
    Parent=FALLBACK/
    '';

    # # You can also set the file content immediately.
    # ".gradle/gradle.properties".text = ''
    #   org.gradle.console=verbose
    #   org.gradle.daemon.idletimeout=3600000
    # '';
  };

  # You can also manage environment variables but you will have to manually
  # source
  #
  #  ~/.nix-profile/etc/profile.d/hm-session-vars.sh
  #
  # or
  #
  #  /etc/profiles/per-user/bcnelson/etc/profile.d/hm-session-vars.sh
  #
  # if you don't want to manage your shell through Home Manager.
  home.sessionVariables = {
    EDITOR = "vim";
    VISUAL = "kwrite";
  };

  # Let Home Manager install and manage itself.
  programs.home-manager.enable = true;
}
