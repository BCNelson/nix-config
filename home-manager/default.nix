{ username }: { inputs, outputs, stateVersion, lib, pkgs, ... }:

{
  nixpkgs = {
    overlays = [
      inputs.nur.overlays.default
      outputs.overlays.unstable-packages
      outputs.overlays.additions
      outputs.overlays.modifications
    ];
    config = {
      allowUnfreePredicate = _pkg: true;
    };
  };

  imports = lib.optional (builtins.pathExists ./${username}) ./${username};
  home.stateVersion = stateVersion;
  # Suppress while nixos-unstable is still on 26.05 and home-manager master is ahead.
  home.enableNixpkgsReleaseCheck = pkgs.lib.trivial.release != "26.05";
  programs.home-manager.enable = true;
  home.username = lib.mkDefault username;
  home.homeDirectory = lib.mkDefault (if pkgs.stdenv.isDarwin then "/Users/${username}" else "/home/${username}");
}
