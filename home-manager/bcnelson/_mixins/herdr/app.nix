{ config, lib, pkgs, ... }:

# Declarative pieces for the Herdr desktop app's home, ~/.config/herdr-app
# (see ./desktop.nix for the app itself). The app treats that folder as its
# whole interface -- layouts, keys, mods -- and redraws from it live, so
# declaring files there is all a mod or layout needs to be "installed".
#
# What the app writes and what it only reads decides how each kind lands:
#
# - mods/<name>/: read only, so plain home-manager links. Each file is linked
#   on its own (recursive) rather than the folder as one store symlink: the
#   app writes jsconfig.json and gpui-kit.d.ts into every mod folder on start,
#   which needs a real, writable directory.
#
# - layouts/<name>.json: read only too. Size, visibility and theme clicks go to
#   layouts/.state/, not the layout file, so a read-only link only costs
#   `layout.set` (editing the tree from inside the app) on these layouts.
#
# - keys.json: the app and its Settings screen rewrite it, and the
#   ctrl-alt-shift-escape reset moves it aside, so it cannot be a link.
#   config-merge renders the declared bindings into it and keeps everything
#   else the app wrote (preserveUnknown), as ./core.nix does for the TUI.
#
# Shipped mods and layouts (main, spaces, menu, ...) are replaced by the app
# on every start, so customizing one means a clone under a new name -- which
# is what the mods and layouts declared here usually are.

let
  cfg = config.local.herdrApp;

  # `herdr-app check` loads the mod and renders it once, failing on errors a
  # typo or an upstream API change would otherwise only show inside the
  # running app. It works headless against an empty app home, so it runs at
  # build time. It writes its type files into the mod folder, so it checks a
  # scratch copy and the installed files stay exactly the source.
  checkedMod = name: src:
    pkgs.runCommand "herdr-app-mod-${name}"
      {
        nativeBuildInputs = [ pkgs.herdr-desktop ];
        # GPUI's text system panics at start without a single font to resolve,
        # and the build sandbox has none. DejaVu Sans is one of its fallbacks.
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
      export HOME=$TMPDIR/home HERDR_APP_HOME=$TMPDIR/app
      mkdir -p "$HERDR_APP_HOME/mods"
      cp -r ${src} "$HERDR_APP_HOME/mods/${name}"
      chmod -R u+w "$HERDR_APP_HOME/mods/${name}"
      herdr-app check "$HERDR_APP_HOME/mods/${name}"
      cp -r ${src} "$out"
    '';

  layoutFile = name: layout:
    if builtins.isAttrs layout
    then (pkgs.formats.json { }).generate "herdr-app-layout-${name}.json" layout
    # Parsed so a malformed file fails evaluation rather than the app keeping
    # its last good version and writing layout.error.
    else builtins.seq (lib.importJSON layout) layout;
in
{
  options.local.herdrApp = {
    mods = lib.mkOption {
      type = lib.types.attrsOf lib.types.path;
      default = { };
      example = lib.literalExpression "{ worktree-main = ./app/mods/worktree-main; }";
      description = ''
        Mod folders (main.js, mod.json, assets) installed as
        ~/.config/herdr-app/mods/<name>. The name is how layouts, keys and
        other mods refer to it. Checked with `herdr-app check` at build time.
      '';
    };

    layouts = lib.mkOption {
      type = lib.types.attrsOf (lib.types.either lib.types.path (lib.types.attrsOf lib.types.anything));
      default = { };
      example = lib.literalExpression "{ my-main = ./app/layouts/my-main.json; }";
      description = ''
        Layouts installed as ~/.config/herdr-app/layouts/<name>.json: a JSON
        file, or the layout tree as a Nix attrset. ctrl-shift-l or
        `layout.switch` shows one; which is shown stays the app's choice.
      '';
    };

    keys = lib.mkOption {
      type = lib.types.attrsOf (lib.types.attrsOf lib.types.str);
      default = { };
      example = { global."ctrl-shift-g" = "mod.open:worktree-main"; };
      description = ''
        Bindings merged into ~/.config/herdr-app/keys.json, by section
        (`global`, `terminal`, `layout:<name>`, `mod:<name>`). Declared keys
        win; every other binding the app or its Settings wrote is kept.
      '';
    };
  };

  config = {
    xdg.configFile =
      lib.mapAttrs'
        (name: src: lib.nameValuePair "herdr-app/mods/${name}" {
          source = checkedMod name src;
          recursive = true;
        })
        cfg.mods
      // lib.mapAttrs'
        (name: layout: lib.nameValuePair "herdr-app/layouts/${name}.json" {
          source = layoutFile name layout;
        })
        cfg.layouts;

    services.config-merge.herdr-app-keys = lib.mkIf (cfg.keys != { }) {
      live = "${config.xdg.configHome}/herdr-app/keys.json";
      settings = cfg.keys;
      # The app writes the rest of this file (its defaults, Settings edits);
      # only the bindings declared above are Nix's.
      preserveUnknown = true;
      # The app watches keys.json and reloads it by itself.
    };
  };
}
