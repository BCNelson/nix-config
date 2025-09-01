# This file defines overlays
{ inputs, ... }:
{
  # This one brings our custom packages from the 'pkgs' directory
  additions = final: _prev: import ../pkgs final.pkgs;

  # This one contains whatever you want to overlay
  # You can change versions, add patches, set compilation flags, anything really.
  # https://nixos.wiki/wiki/Overlays
  modifications = final: prev: {
    # libsForQt5.sddm = nixpkgs-unstable.libsForQt5.sddm;
    claude-code = prev.claude-code.overrideAttrs (_oldAttrs: {
      postInstall = ''
        wrapProgram $out/bin/claude \
          --set DISABLE_AUTOUPDATER 1 \
          --prefix PATH ${final.lib.makeBinPath [
            final.coreutils-full
            final.findutils
            final.gnumake
            final.gnused
            final.gnugrep
            final.bash
            final.ripgrep
          ]}
      '';
    });
  };

  # When applied, the unstable nixpkgs set (declared in the flake inputs) will
  # be accessible through 'pkgs.unstable'
  unstable-packages = final: _prev: {
    unstable = import inputs.nixpkgs-unstable {
      inherit (final) system;
      config = {
        allowUnfree = true;
        permittedInsecurePackages = [
          "electron-25.9.0"
          "libsoup-2.74.3"
        ];
      };
    };
    stable = import inputs.nixpkgs24-05 {
      inherit (final) system;
      config = {
        allowUnfree = true;
      };
    };
  };
}
