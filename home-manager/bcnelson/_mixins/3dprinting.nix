{ pkgs, inputs, ... }:
let
  unstablePkgs = import inputs.nixpkgs-unstable {
    inherit (pkgs.stdenv.hostPlatform) system;
    config = {
      allowUnfree = true;
      permittedInsecurePackages = [ "libsoup-2.74.3" ];
    };
  };
in
{
  nixpkgs.overlays = [
    (_final: _prev: {
      # Self-contained orca-slicer with insecure package allowance
      inherit (unstablePkgs) orca-slicer;
    })
  ];

  home.packages = [
    pkgs.prusa-slicer
    pkgs.orca-slicer
  ];
}