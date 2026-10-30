{ pkgs, lib, ... }:
let
  routingTools = pkgs.kicad-routing-tools;
  # The routing plugin's Python deps have to be on the wrapper's PYTHONPATH,
  # which kicad sets (not prefixes) from pythonPath at build time.
  kicad = pkgs.kicad.overrideAttrs (old: {
    pythonPath = old.pythonPath ++ routingTools.pythonDeps;
  });
in
{
  imports = [
    ../_mixins/programs/libreOffice.nix
    # ../_mixins/programs/emulator.nix
    # ../_mixins/programs/blender.nix
    ./_mixins/programs/veloren.nix
    ./_mixins/workstation.nix
    ./_mixins/mcp/home-assistant.nix
    ./_mixins/3dprinting.nix
    ./_mixins/programs/voxtype.nix
    # Spool replaces Klipper here first (Meta+V, clipboard history).
    ./_mixins/programs/spool.nix
    # Phone/browser access to herdr; served to the tailnet by
    # nixos/sierra/herdr-web-ui.nix.
    ./_mixins/herdr/web-ui.nix
  ];

  home.packages = [
    pkgs.android-tools
    pkgs.nixpkgs-review
    # ventoy-full now lives in nixos/sierra/default.nix (needs root)
    pkgs.libation
    pkgs.kobodl # Kobo ebook library downloader + DRM removal (libation, but for Kobo)
    pkgs.inkscape
    kicad
    pkgs.spec-kit
    pkgs.pince
    # Temporarily dropped: freecad-qt6's dep chain (vtk -> pdal -> gdal) is
    # broken and uncached on nixpkgs-unstable. pdal 2.9.3 fails to compile
    # against gdal 3.13.1 (CSLConstList -> char** in gdal/Raster.cpp:704).
    # Re-enable once nixpkgs fixes the chain. See the gdal deselect in
    # overlays/default.nix which is needed for the minimal gdal test.
    # pkgs.freecad-qt6
  ];

  # KiCad's per-user SWIG plugin dir (PCB Editor > Tools > External Plugins).
  xdg.dataFile."kicad/${lib.versions.majorMinor kicad.version}/scripting/plugins/KiCadRoutingTools".source =
    "${routingTools}/share/kicad/scripting/plugins/KiCadRoutingTools";

  programs.mcp.kubernetes.enable = false;
}
