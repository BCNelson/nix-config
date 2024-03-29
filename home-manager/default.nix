{ username }: { inputs, outputs, stateVersion, lib, pkgs, ... }:

{
  nixpkgs = {
    overlays = [
      inputs.nur.overlay
      outputs.overlays.unstable-packages
    ];
    config = {
      allowUnfreePredicate = _pkg: true;
    };
  };

  imports = lib.optional (builtins.pathExists ./${username}) ./${username};
  home.stateVersion = stateVersion;
  programs.home-manager.enable = true;
  home.username = lib.mkDefault username;
  home.homeDirectory = lib.mkDefault (if pkgs.stdenv.isDarwin then "/Users/${username}" else "/home/${username}");
}
