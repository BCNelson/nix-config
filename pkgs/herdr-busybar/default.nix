{
  buildGoModule,
  lib,
}:
# Shows how many of herdr-core's coding agents are asking for you (blocked),
# working and done, on a BUSY Bar plugged in over USB. Two halves, one package:
#
#   bin/herdr-busybar     the bridge on the laptop (main.go)
#   share/herdr-busybar/  the bar's JavaScript app, packed as the bar's
#                         installer wants it: a .tgz holding the <id>/ folder
#                         (the layout busy-tools' `build` produces)
#
# The bridge installs the app on the bar itself; see
# home-manager/bcnelson/_mixins/herdr/busybar.nix.
buildGoModule {
  pname = "herdr-busybar";
  version = "0.1.0";

  src = ./.;

  # Standard library only.
  vendorHash = null;

  ldflags = [ "-s" "-w" ];

  postInstall = ''
    mkdir -p "$out/share/herdr-busybar"
    # Fixed mtime/owner so the archive is reproducible.
    tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner \
      -czf "$out/share/herdr-busybar/app.tgz" -C app dev.bcnelson.herdr
  '';

  passthru.appPackage = "share/herdr-busybar/app.tgz";

  meta = {
    description = "Counts of herdr-core agents by status, on a BUSY Bar";
    license = lib.licenses.mit;
    mainProgram = "herdr-busybar";
  };
}
