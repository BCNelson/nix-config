# Edit this configuration file to define what should be installed on
# your system.  Help is available in the configuration.nix(5) man page
# and in the NixOS manual (accessible by running ‘nixos-help’).

{ config, pkgs, lib, usernames, ... }:

{
  nix = {
    gc = lib.mkDefault{
      automatic = true;
      dates = "daily";
      options = "--delete-older-than 7d";
    };
    settings = {
      auto-optimise-store = true;
      experimental-features = [ "nix-command" "flakes" ];

      # Allow bcnelson to add substituters / push to the store without sudo.
      trusted-users = [ "root" "@wheel" "bcnelson" ];

      # Avoid unwanted garbage collection when using nix-direnv
      keep-outputs = true;
      keep-derivations = true;

      warn-dirty = false;

      extra-substituters = [
        "https://devenv.cachix.org"
        # Prebuilt authentik (frontend/rust/gopkgs/etc.) from authentik-nix CI,
        # so RAM-constrained hosts like whiskey don't OOM building it from source.
        "https://nix-community.cachix.org"
      ];
      extra-trusted-public-keys = [
        "devenv.cachix.org-1:w1cLUi8dv3hnoSPGAuibQv+f9TZLr6cv/Hm9XgU50cw="
        "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
      ];
    };
  };

  # Bootloader.
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;

  # Enable networking
  networking.networkmanager.enable = true;

  # Set your time zone.
  time.timeZone = lib.mkDefault "America/Denver";

  # Select internationalisation properties.
  i18n.defaultLocale = "en_US.UTF-8";

  # Add this to ensure all required locales are generated
  i18n.supportedLocales = [
    "en_US.UTF-8/UTF-8"
    "C.UTF-8/UTF-8"  # Add basic C locale as fallback
  ];

  # Add this to ensure all environment variables are set
  environment.variables = {
    LANG = "en_US.UTF-8";
    LC_ALL = "en_US.UTF-8";
  };

  i18n.extraLocaleSettings = {
    LC_ADDRESS = "en_US.UTF-8";
    LC_IDENTIFICATION = "en_US.UTF-8";
    LC_MEASUREMENT = "en_US.UTF-8";
    LC_MONETARY = "en_US.UTF-8";
    LC_NAME = "en_US.UTF-8";
    LC_NUMERIC = "en_US.UTF-8";
    LC_PAPER = "en_US.UTF-8";
    LC_TELEPHONE = "en_US.UTF-8";
    LC_TIME = "en_US.UTF-8";
    LC_ALL = "en_US.UTF-8";
  };

  security.rtkit.enable = true;

  programs.dconf.enable = lib.mkDefault true;

  # A thin client never touches the repo -- it pulls prebuilt closures instead of
  # rebuilding -- so the git tooling there is dead weight, and it drags perl in
  # behind it. See nixos/_mixins/roles/thin-client/minimal.nix.
  environment.systemPackages = with pkgs; [
    nano
  ] ++ lib.optionals (!config.services.bcnelson.thinClient.enable) [
    git
    git-crypt
    powertop
    # System-wide rather than in bcnelson's profile because `herdr --remote`
    # resolves the far end with `command -v herdr` over a non-interactive ssh
    # command, which never sources the login shell that would put
    # ~/.nix-profile/bin on PATH. /run/current-system/sw/bin is always there.
    # The matching user config comes from
    # home-manager/bcnelson/_mixins/herdr/core.nix.
    herdr
  ];

  # tmux only survives where it is still somebody's multiplexer. bcnelson moved
  # to herdr, so a host where he is the only user has nothing left to attach to.
  # Two exceptions keep it:
  #   - thin clients, which do not get herdr at all (2 GB of RAM, and a login
  #     there is for reading a journal rather than holding a session), and
  #   - any host with a second user, since nobody else moved off it. hlnelson
  #     also keeps the home-manager side, imported directly in
  #     home-manager/hlnelson/default.nix now that _mixins/console/full.nix no
  #     longer hands it to everyone.
  programs.tmux.enable =
    if (usernames == [ "bcnelson" ] && !config.services.bcnelson.thinClient.enable)
    then false
    else lib.mkDefault true;

  services.fwupd.enable = lib.mkDefault true;
}
