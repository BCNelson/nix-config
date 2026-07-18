{ pkgs, ... }:

{
  imports = [
    ../_mixins/programs/libreOffice.nix
    # ../_mixins/programs/emulator.nix
    # ../_mixins/programs/blender.nix
    ./_mixins/programs/veloren.nix
    ./_mixins/workstation.nix
    ./_mixins/3dprinting.nix
  ];

  home.packages = [
    pkgs.android-tools
    pkgs.nixpkgs-review
    # pkgs.ventoy-full
    pkgs.libation
    pkgs.inkscape
    pkgs.spec-kit
    pkgs.pince
    # Temporarily dropped: freecad-qt6's dep chain (vtk -> pdal -> gdal) is
    # broken and uncached on nixpkgs-unstable. pdal 2.9.3 fails to compile
    # against gdal 3.13.1 (CSLConstList -> char** in gdal/Raster.cpp:704).
    # Re-enable once nixpkgs fixes the chain. See the gdal deselect in
    # overlays/default.nix which is needed for the minimal gdal test.
    # pkgs.freecad-qt6
  ];
}
