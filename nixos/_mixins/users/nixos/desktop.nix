{ config, desktop, lib, pkgs, ... }: {
  config.environment.systemPackages = with pkgs; [
    gparted
  ];
  config.systemd.tmpfiles.rules = [
    "d /home/nixos/Desktop 0755 nixos users"
    "L+ /home/nixos/Desktop/gparted.desktop - - - - ${pkgs.gparted}/share/applications/gparted.desktop"
    "L+ /home/nixos/Desktop/io.elementary.terminal.desktop - - - - ${pkgs.pantheon.elementary-terminal}/share/applications/io.elementary.terminal.desktop"
    "L+ /home/nixos/Desktop/io.calamares.calamares.desktop - - - - ${pkgs.calamares-nixos}/share/applications/io.calamares.calamares.desktop"
  ];
  config.isoImage.edition = lib.mkForce "${desktop}";
  config.services.displayManager.autoLogin.user = "nixos";
  config.services.kmscon.autologinUser = lib.mkForce null;
}
