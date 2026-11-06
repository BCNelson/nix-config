{ config, lib, pkgs, ... }:
let
  cfg = config.hardware.graphics;
  # Same shape as NixOS's hardware.graphics: one env holding the userspace
  # drivers, exposed at /run/opengl-driver.
  driverEnv = pkgs.buildEnv {
    name = "opengl-drivers";
    paths = [ pkgs.mesa ] ++ cfg.extraPackages;
  };
in
{
  # system-manager counterpart of NixOS's `hardware.graphics.enable`, named to
  # match. Only the /run/opengl-driver half: Fedora's own Mesa already serves
  # its native apps, and nix apps normally get Mesa from the home-manager
  # nixGL wrap (LD_LIBRARY_PATH).
  #
  # That wrap does not survive into buildFHSEnv apps. zoom-us is the case that
  # broke: its launcher re-execs the client with its own LD_LIBRARY_PATH
  # (/opt/zoom/Qt/lib:...), dropping nixGL's, so the FHS's libglvnd finds no
  # libGLX_mesa. Qt then qFatal()s with "Could not initialize GLX", and zoom's
  # crash handler overflows its 8 KiB sigaltstack inside g_spawn_sync, which
  # is the misleading SIGSEGV-in-read_data that coredumpctl reports. The FHS
  # ld.so.conf already lists /run/opengl-driver/lib (it is how this works on
  # NixOS), so providing that path is enough -- no per-app wrapping.
  options.hardware.graphics = {
    enable = lib.mkEnableOption ''
      a {file}`/run/opengl-driver` symlink to Mesa, for nix apps (notably
      buildFHSEnv ones) that look for GL/EGL/GBM drivers there
    '';

    extraPackages = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ ];
      description = "Additional driver packages to add to {file}`/run/opengl-driver`.";
    };
  };

  config = lib.mkIf cfg.enable {
    # L+ replaces whatever is there, so a new Mesa takes over on activation;
    # /run is a tmpfs, so systemd-tmpfiles-setup recreates it at boot.
    systemd.tmpfiles.rules = [ "L+ /run/opengl-driver - - - - ${driverEnv}" ];
  };
}
