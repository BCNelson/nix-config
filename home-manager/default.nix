{username}: { stateVersion, lib, pkgs, ... }:

{
  imports = lib.optional (builtins.pathExists ./${username}) ./${username};
  home.stateVersion = stateVersion;
  programs.home-manager.enable = true;
  home.username = lib.mkDefault username;
  home.homeDirectory = lib.mkDefault (if pkgs.stdenv.isDarwin then "/Users/${username}" else "/home/${username}");
}
