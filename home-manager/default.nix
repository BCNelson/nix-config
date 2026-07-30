{ username }: { inputs, outputs, stateVersion, genericLinux ? false, lib, pkgs, ... }:

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

  # `targets.genericLinux` exists for running home-manager on a distro that is
  # not NixOS. Its GPU shim (non-nixos-gpu -> mesa -> llvm) is ~850 MB, and on
  # NixOS it is shimming around a problem that does not exist -- NixOS provides
  # the GL stack itself. Correct for mkStandaloneHome and mkSystemManager, which
  # both target foreign distros; wrong everywhere else. mkDefault so an
  # individual user can still override.
  targets.genericLinux.enable = lib.mkDefault genericLinux;
  home.username = lib.mkDefault username;
  home.homeDirectory = lib.mkDefault (if pkgs.stdenv.isDarwin then "/Users/${username}" else "/home/${username}");
}
