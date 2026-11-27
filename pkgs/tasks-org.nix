{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  makeWrapper,
  alsa-lib,
  fontconfig,
  freetype,
  glib,
  libGL,
  libxkbcommon,
  wayland,
  libx11,
  libxext,
  libxi,
  libxrender,
  libxtst,
  libxcursor,
  libxrandr,
  zlib,
  libnotify,
  gtk3,
  xdg-utils,
  xrdb,
  gawk,
  writeShellScript,
}:
# Tasks.org desktop (Compose Multiplatform / JVM), not in nixpkgs. Upstream
# ships it with Hydraulic Conveyor as a self-contained tree: a native
# launcher, ~200 jars and a bundled JetBrains Runtime (with its Wayland AWT
# toolkit). We use the versioned tarball rather than the .deb or the
# AppImage, whose URL has no version in it and so can't be pinned.
#
# The launcher finds lib/app and lib/runtime relative to its own path, so the
# tree is kept intact under $out/lib/tasks-org and autoPatchelf'd in place.
let
  sources = {
    x86_64-linux = {
      arch = "amd64";
      hash = "sha256-9Z3JcqmxlDG0I50aQ3DgmIIEnmripsN9VwmnD463ngM=";
    };
    aarch64-linux = {
      arch = "aarch64";
      hash = "sha256-cr7R/k6/fE7mo3q9zBkEdkJCvzMhwRohfIvAON63jMc=";
    };
  };
  source = sources.${stdenv.hostPlatform.system} or (throw "tasks-org: unsupported system ${stdenv.hostPlatform.system}");

  # The app runs on X11 (XWayland), where Java draws at 1x regardless of the
  # desktop's fractional scale: sharp but small. Native Wayland isn't an
  # option -- the launcher burns its JVM flags in and drops JAVA_TOOL_OPTIONS,
  # and even on nixpkgs' JBR with -Dawt.toolkit.name=WLToolkit, Compose 1.12's
  # window dies with "Can't lock DrawingSurface" (Skiko needs JAWT, which
  # WLToolkit lacks). So derive the scale at launch from Xft.dpi, which KDE
  # (with "apply scaling themselves") and most desktops set to 96 x scale,
  # and hand it to Java's X11 backend via J2D_UISCALE. Xft.dpi is an integer
  # (1.3 -> 124), so round to the nearest 0.05. Prints nothing at 1x or when
  # there's no X server, leaving Java's own default alone.
  # Java's Desktop.browse() reports BROWSE unsupported outside GNOME, so the
  # app falls back to running `xdg-open <url>` (DesktopBrowseKt). That child
  # would inherit our LD_LIBRARY_PATH (nixpkgs GTK et al.) and hand it on to
  # the browser, so put a shim first on PATH that strips the app-only
  # environment before calling the real xdg-open. The original
  # LD_LIBRARY_PATH is saved by the wrapper in TASKS_ORG_ORIG_LD_LIBRARY_PATH.
  xdgOpenShim = writeShellScript "xdg-open" ''
    if [ -n "''${TASKS_ORG_ORIG_LD_LIBRARY_PATH+x}" ]; then
      export LD_LIBRARY_PATH="$TASKS_ORG_ORIG_LD_LIBRARY_PATH"
      [ -n "$LD_LIBRARY_PATH" ] || unset LD_LIBRARY_PATH
    fi
    unset TASKS_ORG_ORIG_LD_LIBRARY_PATH J2D_UISCALE
    exec ${xdg-utils}/bin/xdg-open "$@"
  '';

  detectScale = writeShellScript "tasks-org-detect-scale" ''
    ${xrdb}/bin/xrdb -query 2>/dev/null \
      | ${gawk}/bin/awk '/^Xft\.dpi:/ { s = int($2 / 96 * 20 + 0.5) / 20; if (s > 1) print s; exit }'
  '';
in
  stdenv.mkDerivation rec {
    pname = "tasks-org";
    version = "15.14.1";

    src = fetchurl {
      url = "https://update.tasks.org/tasks-org-${version}-linux-${source.arch}.tar.gz";
      inherit (source) hash;
    };

    nativeBuildInputs = [autoPatchelfHook makeWrapper];

    buildInputs = [
      alsa-lib
      fontconfig
      freetype
      glib
      libGL
      libxkbcommon
      wayland
      libx11
      libxext
      libxi
      libxrender
      libxtst
      zlib
      stdenv.cc.cc.lib
    ];

    # Loaded with dlopen (not DT_NEEDED), so autoPatchelf can't see them: AWT
    # pulls in Xcursor/Xrandr, Skia's OpenGL backend wants libGL, desktop
    # notifications go through libnotify, and java.awt.Desktop.browse() (OAuth
    # sign-in, links) goes through GTK -- without it, opening a browser fails
    # silently.
    runtimeLibs = [libGL libxcursor libxrandr libnotify gtk3];

    installPhase = ''
      runHook preInstall

      mkdir -p $out/lib/tasks-org $out/bin
      cp -r bin lib $out/lib/tasks-org/
      cp -r share $out/

      mkdir -p $out/lib/tasks-org/shims
      ln -s ${xdgOpenShim} $out/lib/tasks-org/shims/xdg-open

      makeWrapper $out/lib/tasks-org/bin/tasks-org $out/bin/tasks-org \
        --run 'export TASKS_ORG_ORIG_LD_LIBRARY_PATH="''${LD_LIBRARY_PATH-}"' \
        --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath runtimeLibs} \
        --prefix PATH : $out/lib/tasks-org/shims \
        --run 'if [ -z "''${J2D_UISCALE-}" ]; then s=$(${detectScale}); [ -n "$s" ] && export J2D_UISCALE="$s"; fi'

      substituteInPlace $out/share/applications/*.desktop \
        --replace-fail "Exec=/usr/lib/tasksorg-llc/tasks-org/bin/tasks-org" "Exec=$out/bin/tasks-org"

      runHook postInstall
    '';

    meta = {
      description = "Tasks.org desktop: to-do lists synced with Tasks.org, Google Tasks, CalDAV and EteSync";
      homepage = "https://tasks.org/";
      license = lib.licenses.gpl3Only;
      sourceProvenance = [lib.sourceTypes.binaryNativeCode lib.sourceTypes.binaryBytecode];
      mainProgram = "tasks-org";
      platforms = builtins.attrNames sources;
    };
  }
