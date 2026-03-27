{ pkgs, ... }:
let
  inherit (pkgs.unstable) mesa;
in
{
  programs.vscode = {
    enable = true;
    # nixGL.wrap is incompatible with FHS bubblewrap (LD_LIBRARY_PATH conflicts).
    # Instead, inject GPU libs inside the FHS namespace and set the env vars that
    # nixGL normally provides so libglvnd can find the DRI drivers.
    package = let
      base = pkgs.unstable.vscode.fhsWithPackages (ps: with ps; [
        mesa
        libGL
        libglvnd
        vulkan-loader
      ]);
    in (pkgs.symlinkJoin {
      name = base.name or "vscode";
      paths = [ base ];
      nativeBuildInputs = [ pkgs.makeWrapper ];
      postBuild = ''
        for f in $out/bin/*; do
          wrapProgram "$f" \
            --set LIBGL_DRIVERS_PATH "${mesa}/lib/dri" \
            --set GBM_BACKENDS_PATH "${mesa}/lib/gbm" \
            --set __EGL_VENDOR_LIBRARY_FILENAMES "${mesa}/share/glvnd/egl_vendor.d/50_mesa.json"
        done
      '';
    }) // {
      pname = base.pname or "code";
      version = base.version or pkgs.unstable.vscode.version;
      meta = (base.meta or {}) // { mainProgram = "code"; };
    };
    profiles.default.userSettings = {
      "terminal.integrated.defaultProfile.linux" = "fish";
      "editor.inlineSuggest.enabled" = true;
      "github.copilot.enable" = {
        "*" = true;
      };
      "editor.fontFamily" = "'Monaspace Neon', 'monospace', monospace";
      "update.mode" = "none";
    };
  };

  home.file = {
    codeConfig = {
      enable = false;
      target = ".config/code-flags.conf";
      text = ''
            '';
    };
  };

  programs.fish.functions = {
    zcode = {
      body = ''
        set -l path (z -e $argv)
        direnv exec $path code $path
      '';
    };
  };
}
