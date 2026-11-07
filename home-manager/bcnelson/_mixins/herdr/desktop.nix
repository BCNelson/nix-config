{ lib, pkgs, genericLinux, ... }:

# Herdr desktop app (https://github.com/herdrdev/herdr-app) -- on trial
# alongside the herdr TUI, not replacing it. The two do not share anything: the
# app is a client of herdr-core (`hc`), a separate runtime with its own
# terminals, machines and ~/.config/herdr-core, so nothing in ./default.nix or
# ./core.nix (agent hooks, the workspace layout, web-ui, mirror, the
# herdr-server units) applies to it. Sessions started in one do not appear in
# the other.
#
# pkgs.herdr-desktop already points the app at pkgs.herdr-core and turns off
# the self-updater, so the app starts that core itself when none is running.
#
# Two settings go on a wrapper rather than in settings.json, which the app
# rewrites itself (a home-manager symlink there would be read-only):
#
# - VK_DRIVER_FILES, on a non-NixOS host only. The app renders through
#   wgpu/Vulkan, and the Vulkan loader otherwise picks up the distro's ICD
#   manifests and fails to dlopen them against Nix's libraries (on redo-3:
#   libLLVM.so.21.1, libz, libdrm not found). Pointing it at Nix's Mesa ICD
#   is all it takes -- the driver has its own RUNPATH and the app already
#   carries the loader in its RUNPATH. This is deliberately not
#   config.lib.nixGL.wrap or nixVulkanIntel as zed.nix uses: those export
#   LD_LIBRARY_PATH, and the app starts herdr-core with its own environment,
#   which core hands to every terminal it opens. Nix library paths in every
#   shell would break the distro's own binaries; this one variable only
#   steers Vulkan programs onto Nix's RADV, which works for them too.
#   Only radeon is listed because redo-3 is the one genericLinux host and it
#   has a Radeon 780M; add the ICD for any other GPU before importing this
#   on another non-NixOS machine.
#
# - HERDR_TELEMETRY=0. Usage telemetry is on by default; per upstream's
#   docs/security.md a source build like this one should not send it anyway,
#   but the variable makes that explicit rather than an accident of how it
#   was built.
let
  vulkanDrivers = "${pkgs.mesa}/share/vulkan/icd.d/radeon_icd.${pkgs.stdenv.hostPlatform.parsed.cpu.name}.json";

  herdr-desktop = pkgs.symlinkJoin {
    name = "herdr-desktop-wrapped-${pkgs.herdr-desktop.version}";
    paths = [ pkgs.herdr-desktop ];
    nativeBuildInputs = [ pkgs.makeWrapper ];
    # The .desktop entry runs a bare `herdr-app`, which depends on the
    # launcher's PATH reaching the profile; point it at the wrapper outright.
    postBuild = ''
      wrapProgram $out/bin/herdr-app \
        --set HERDR_TELEMETRY 0 ${lib.optionalString genericLinux "--set VK_DRIVER_FILES ${vulkanDrivers}"}

      desktop=share/applications/herdr-app.desktop
      rm $out/$desktop
      substitute ${pkgs.herdr-desktop}/$desktop $out/$desktop \
        --replace-fail "Exec=herdr-app" "Exec=$out/bin/herdr-app"
    '';
    inherit (pkgs.herdr-desktop) meta;
  };
in
{
  # `hc` on PATH too (from ./hc.nix, with its Tailscale transport), so the app
  # finds the same core from a terminal and agents can be pointed at
  # `hc --skill`.
  imports = [ ./hc.nix ];

  home.packages = [ herdr-desktop ];
}
