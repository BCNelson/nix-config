{ pkgs, inputs, ... }:
let
  unstablePkgs = import inputs.nixpkgs-unstable {
    inherit (pkgs) system;
    config = {
      allowUnfree = true;
      permittedInsecurePackages = [ "libsoup-2.74.3" ];
    };
  };
in
{
  nixpkgs.overlays = [
    (final: prev: {
      # Self-contained orca-slicer with insecure package allowance
      orca-slicer = unstablePkgs.orca-slicer;
    })
  ];

  home.packages = [
    pkgs.prusa-slicer
    pkgs.orca-slicer
  ];
}