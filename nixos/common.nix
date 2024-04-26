# Edit this configuration file to define what should be installed on
# your system.  Help is available in the configuration.nix(5) man page
# and in the NixOS manual (accessible by running ‘nixos-help’).

{ pkgs, lib, ... }:

{
  nix = {
    gc = {
      automatic = true;
      dates = "weekly";
      options = "--delete-older-than 7d";
    };
    settings = {
      auto-optimise-store = true;
      experimental-features = [ "nix-command" "flakes" ];

      # Avoid unwanted garbage collection when using nix-direnv
      keep-outputs = true;
      keep-derivations = true;

      warn-dirty = false;
    };
  };

  # Bootloader.
  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;

  # Enable networking
  networking.networkmanager.enable = true;
  networking.wireless.enable = lib.mkForce false;

  # Set your time zone.
  time.timeZone = lib.mkDefault "America/Denver";

  # Select internationalisation properties.
  i18n.defaultLocale = "en_US.UTF-8";

  i18n.supportedLocales = lib.mkDefault [
    "en_US.UTF-8/UTF-8"
  ];

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
  };

  security.rtkit.enable = true;

  programs.dconf.enable = true;

  environment.systemPackages = with pkgs; [
    vim
    git
    git-crypt
    pciutils
    waypipe
  ];

  programs.tmux.enable = true;

  programs.fish = {
    enable = true;
    useBabelfish = true;
  };
  services.fwupd.enable = true;
}
