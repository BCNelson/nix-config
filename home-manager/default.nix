{ outputs, username, stateVersion, lib, ... }:

{
  imports = [] ++ lib.optional (builtins.pathExists ./${username}) ./${username};
  home.stateVersion = stateVersion;
  programs.home-manager.enable = true;
}
