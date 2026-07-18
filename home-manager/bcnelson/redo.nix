{ config, pkgs, ... }:

{
  imports = [
    ../_mixins/work/redo.nix
    ./_mixins/kwin-adaptive-workspaces.nix
    ./_mixins/power-saver-refresh.nix
    ./_mixins/workstation.nix
    ./_mixins/mcp/aws/support.nix
    ./_mixins/mcp/aws/cloudwatch.nix
    ./_mixins/mcp/datadog.nix
    ./_mixins/mcp/redo-production-crdb.nix
    ./_mixins/mcp/mongodb-redo-production.nix
  ];

  home.packages = [
    (config.lib.nixGL.wrap pkgs.winbox4)
    pkgs.gam # Google Workkspace CLI
    (config.lib.nixGL.wrap pkgs.devpod-desktop)
    pkgs.coder
    pkgs.robocode # Robocode Tank Royale
  ];

  programs.plasma = {
    enable = true;
    kwin.virtualDesktops = {
      names = [ "Left" "Main" "Right" ];
      rows = 1;
    };
  };

  # nixGL exports LIBGL_DRIVERS_PATH / GBM_BACKENDS_PATH / LD_LIBRARY_PATH
  # pointing into /nix/store, which Flatpak forwards into the sandbox where
  # those paths don't resolve. Flatpak Zoom's bundled Qt6 aborts during
  # GL/EGL init because of it; other Flatpaks tolerate it today but shouldn't
  # have to. Strip them via a global per-user Flatpak override.
  home.file.".local/share/flatpak/overrides/global".text = ''
    [Environment]
    unset-environment=LIBGL_DRIVERS_PATH;GBM_BACKENDS_PATH;__EGL_VENDOR_LIBRARY_FILENAMES;LIBVA_DRIVERS_PATH;LD_LIBRARY_PATH
  '';
}
