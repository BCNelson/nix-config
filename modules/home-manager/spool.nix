# Spool: clipboard history for KDE Plasma 6 on Wayland (pkgs/spool).
#
# Runs `spoold` as a hardened systemd user service tied to the graphical
# session and writes its policy file, ~/.config/spool/config.toml.
#
# The unit itself lives in ../../pkgs/spool/systemd-service.nix so the VM test
# (`nix build .#spool-test`) runs exactly the same sandbox; that file also
# records which sandboxing directives take effect in a *user* unit and why.
#
# Import with `outputs.homeModules.spool` (see ./default.nix).
{
  config,
  lib,
  options,
  pkgs,
  ...
}: let
  cfg = config.services.spool;
  toml = pkgs.formats.toml {};

  unitDef = import ../../pkgs/spool/systemd-service.nix {
    inherit lib;
    inherit (cfg) package pickerPackage;
  };

  # Keys spool_core::config::Config accepts. Config is `deny_unknown_fields`
  # (also inside [picker]), so writing a key the daemon doesn't know makes
  # spoold refuse to start -- only these may be written.
  supported = {
    max_age_days = cfg.retention.maxAgeDays;
    max_items = cfg.retention.maxItems;
    primary_selection = cfg.primarySelection;
    excluded_apps = cfg.excludedApps;
    key_provider = cfg.keyProvider;
    auto_paste = cfg.autoPaste;
    # A list replaces spoold's built-in one (this option defaults to it).
    paste_terminals = cfg.pasteTerminals;
    # [picker]: only policy; the executable comes from the unit's PATH.
    picker = {
      inherit (cfg.picker) renderer prerender;
    };
    # [editor]: commands for "edit in external editor". The one exception to
    # "policy only" (see pathLikeKeys): spoold never runs these itself; each
    # session is a transient systemd user unit outside spoold's sandbox, i.e.
    # exactly what any process of this user could start anyway.
    editor = {
      inherit (cfg.editor) terminal mime;
    };
  };

  # The user unit does not see the shell's $EDITOR, so the default text
  # editor comes from home.sessionVariables (VISUAL, then EDITOR), else
  # Neovim; (n)vim gets flags that keep the edited text out of swap, undo,
  # backup and shada files.
  sessionEditor = let
    v = config.home.sessionVariables;
  in
    v.VISUAL or v.EDITOR or null;
  editorWords =
    if sessionEditor == null
    then ["nvim"]
    else lib.filter (s: s != "") (lib.splitString " " (toString sessionEditor));
  editorName = baseNameOf (builtins.head editorWords);
  # Run in `{terminal}`; anything else is taken to be a GUI editor.
  terminalEditors = ["nvim" "vim" "vi" "nano" "micro" "hx" "helix" "kak" "joe" "ne" "mg"];
  # GUI editors that need a flag to wait until the file is closed.
  waitFlags = {
    kate = ["--block"];
    code = ["--wait"];
    codium = ["--wait"];
    subl = ["--wait"];
    zed = ["--wait"];
    gedit = ["--wait"];
  };
  vimHardening = ["-n" "-i" "NONE" "--cmd" "set noundofile nobackup nowritebackup"];
  defaultTextEditor =
    if builtins.elem editorName terminalEditors
    then
      ["{terminal}"]
      ++ editorWords
      ++ lib.optionals (builtins.elem editorName ["nvim" "vim"]) vimHardening
      ++ ["{file}"]
    else
      editorWords
      ++ lib.filter (f: !(builtins.elem f editorWords)) (waitFlags.${editorName} or [])
      ++ ["{file}"];

  # spool-paste's DEFAULT_TERMINALS (crates/spool-paste/src/chord.rs): KWin
  # desktop-file app ids that paste with Ctrl+Shift+V.
  defaultPasteTerminals = [
    "org.kde.konsole"
    "com.mitchellh.ghostty"
    "kitty"
    "foot"
    "footclient"
    "Alacritty"
    "org.wezfurlong.wezterm"
  ];

  rendered = lib.recursiveUpdate supported cfg.settings;
  configFile = toml.generate "spool-config.toml" rendered;

  # The config file is policy only: spoold must never learn an executable or
  # socket path from a file in $HOME (executables come from the unit's PATH,
  # the socket from $XDG_RUNTIME_DIR). Reject anything that looks like one.
  # Exception: [editor.mime] (mime-type keys -> editor argv, see `editor`
  # above), skipped here because a mime like application/x-binary would
  # trip the key check.
  pathLikeKeys = let
    bad = k: builtins.match ".*(path|socket|sock|exec|command|cmd|bin|program).*" (lib.toLower k) != null;
    walk = prefix: attrs:
      lib.concatLists (lib.mapAttrsToList (k: v: let
        name = "${prefix}${k}";
      in
        lib.optional (bad k) name ++ lib.optionals (builtins.isAttrs v) (walk "${name}." v))
      attrs);
  in
    walk "" (rendered // {editor = removeAttrs (rendered.editor or {}) ["mime"];});

  # plasma-manager (inputs.plasma-manager) is how this repo manages KDE
  # config; use it when the importing config has it, else fall back to
  # kwriteconfig6.
  hasPlasmaManager = options ? programs.plasma.shortcuts;

  # Plasma 6's Meta+V is Klipper's "Show Clipboard Items at Mouse Position",
  # which since Plasma 6 lives in plasmashell: kglobalshortcutsrc
  # [plasmashell] show-on-mouse-pos. Spool's KWin script registers Meta+V
  # itself ([kwin] spool-show), and kglobalaccel refuses a key that another
  # action already holds, so Klipper's binding must go.
  klipperGroup = "plasmashell";
  klipperAction = "show-on-mouse-pos";
in {
  options.services.spool = {
    enable = lib.mkEnableOption "Spool, a clipboard history daemon for KDE Plasma 6 on Wayland";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.spool;
      defaultText = lib.literalExpression "pkgs.spool";
      description = "Package providing `spoold` and `spoolctl`.";
    };

    pickerPackage = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = pkgs.spool-picker or null;
      defaultText = lib.literalExpression "pkgs.spool-picker or null";
      description = ''
        Package providing `spool-picker`, put on spoold's (otherwise minimal)
        PATH. `null` runs the daemon without a picker.
      '';
    };

    keyProvider = lib.mkOption {
      type = lib.types.enum ["secret-service" "session"];
      default = "secret-service";
      description = ''
        Where the history data key comes from: `secret-service` (a key slot
        in KWallet / gnome-keyring via org.freedesktop.secrets; history is
        persisted encrypted) or `session` (memory only; history is lost when
        spoold exits).
      '';
    };

    primarySelection = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Also record the primary selection (middle-click buffer).";
    };

    retention = {
      maxAgeDays = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 14;
        description = "Unpinned items not used for this many days are deleted.";
      };
      maxItems = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 5000;
        description = "Keep at most this many unpinned items.";
      };
    };

    excludedApps = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      example = ["org.keepassxc.KeePassXC"];
      description = "App ids whose copies are never recorded (case-insensitive exact match).";
    };

    autoPaste = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        After an item is picked with Enter, press the paste chord in the
        window that was focused when the picker opened (through the
        `spool-paster` helper). `false`: picking only sets the clipboard.
      '';
    };

    pasteTerminals = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = defaultPasteTerminals;
      description = ''
        App ids that auto-paste with Ctrl+Shift+V instead of Ctrl+V. Written
        as `paste_terminals`, which replaces spoold's built-in list (the
        default here is that same list).
      '';
    };

    picker = {
      prerender = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Keep the resident picker's next frame drawn while it is hidden, so
          Meta+V shows it in ~1-2 ms. `false` frees the frame buffers while
          hidden (a little slower to show).
        '';
      };
      renderer = lib.mkOption {
        type = lib.types.enum ["software" "gpu"];
        default = "software";
        description = ''
          Picker renderer. `gpu` (FemtoVG/EGL) falls back to software when EGL
          is missing or a software GL. Drivers that JIT on the CPU (llvmpipe)
          crash under the unit's MemoryDenyWriteExecute; spoold relaunches a
          `gpu` picker that dies before it is ready with `software`.
        '';
      };
    };

    editor = {
      terminal = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = ["konsole" "--separate" "-e"];
        example = ["foot"];
        description = ''
          Terminal command spliced in for a `{terminal}` argument of
          `editor.mime` (it must run the rest of the arguments as a command
          and stay open until it exits). Konsole needs `--separate`: without
          it a second `konsole` may hand the window to a running instance and
          exit at once, which Spool would take as "editor closed".
        '';
      };

      mime = lib.mkOption {
        type = lib.types.attrsOf (lib.types.listOf lib.types.str);
        default = {};
        example = lib.literalExpression ''
          {
            "text/*" = ["{terminal}" "nvim" "-n" "-i" "NONE" "{file}"];
            "text/html" = ["kate" "--block" "{file}"];
            "image/*" = ["krita" "--nosplash" "{file}"];
          }
        '';
        description = ''
          Editors for "edit in external editor" (Ctrl+E / Ctrl+Shift+E in the
          picker, `spoolctl edit`, `spoolctl new`), as `[editor.mime]`: a mime
          pattern (exact, `type/*` or `*/*`; the most specific match wins) to
          an argv with `{file}` (the temp file) and optionally `{terminal}`.
          The command must block until you are done editing (`kate --block`,
          `code --wait`, a terminal editor); each save is stored as a new
          item and the last one goes on the clipboard when it exits.

          `"text/*"` defaults (`mkDefault`) to `home.sessionVariables.VISUAL`
          or `EDITOR` if set, else `nvim`: terminal editors (nvim, vim, nano,
          helix, ...) run in `{terminal}`, (n)vim with `-n -i NONE --cmd
          "set noundofile nobackup nowritebackup"`; other (GUI) editors run
          directly, with `--block` / `--wait` added for kate, code, codium,
          subl, zed and gedit. spoold runs in a systemd user unit, so a
          shell-only `$EDITOR` (set in .bashrc, not in sessionVariables) is
          not seen.

          The edited plaintext lives in `$XDG_RUNTIME_DIR/spool/edit` (tmpfs,
          0700) and is deleted when the editor exits, but the editor itself
          runs outside Spool's control: it may keep swap, undo, backup,
          recent-files or crash-recovery copies elsewhere (e.g. Neovim's
          ~/.local/state/nvim/{swap,undo,shada}, Kate's session data,
          Krita's autosave). Configure it not to, as the default does for
          (n)vim.
        '';
      };
    };

    disableKlipperShortcut = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Unbind Plasma's own Meta+V (Klipper's "Show Clipboard Items at Mouse
        Position", `[plasmashell] show-on-mouse-pos` in kglobalshortcutsrc) so
        Spool's KWin script can take the key. Uses plasma-manager when it is
        imported, otherwise `kwriteconfig6` at activation. Takes effect at the
        next login (kglobalaccel reads it at startup).
      '';
    };

    disableKlipperHistory = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Stop Klipper (Plasma's built-in clipboard history) from keeping its
        own history and from re-asserting the clipboard when the owning app
        exits, so Spool is the only clipboard manager. Writes klipperrc via
        plasma-manager; no-op without it. Takes effect at the next login.
      '';
    };

    settings = lib.mkOption {
      inherit (toml) type;
      default = {};
      example = {
        max_rep_bytes = 4194304;
        mime_allowlist = ["text/plain;charset=utf-8" "text/plain"];
      };
      description = ''
        Extra spool-core Config keys, merged last into config.toml. spoold
        rejects unknown keys, and the file must never carry executable or
        socket paths.
      '';
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      assertions = [
        {
          assertion = pathLikeKeys == [];
          message = "services.spool: config.toml is policy only; remove path-like keys: ${lib.concatStringsSep ", " pathLikeKeys}";
        }
      ];

      # spoolctl on PATH, and the package's share/applications in
      # XDG_DATA_DIRS: dev.bcnelson.spool.paster.desktop is how KWin grants
      # the spool-paster helper org_kde_kwin_fake_input (it matches the
      # desktop file's Exec= path; spoold itself is non-dumpable, so KWin can
      # never match it), and dev.bcnelson.spool.daemon.desktop is the portal
      # app id.
      home.packages = [cfg.package];

      xdg.configFile."spool/config.toml".source = configFile;

      services.spool.editor.mime."text/*" = lib.mkDefault defaultTextEditor;

      systemd.user.services.spool = {
        Unit =
          unitDef.unit
          // {
            # Restart on config changes when home-manager switches.
            X-Restart-Triggers = ["${configFile}"];
          };
        Service =
          unitDef.service
          // {
            Environment = ["PATH=${unitDef.path}"];
            # KWin checks the spool-paster fake_input grant against KDE's
            # service cache (ksycoca), which otherwise only refreshes at login,
            # so after a rebuild it still lists the old store path and refuses
            # auto-paste. Refresh it here: the user manager carries the Plasma
            # session's XDG_DATA_DIRS, which names the cache file KWin reads
            # (home-manager's activation service has a different one). "-":
            # never block spoold on it.
            ExecStartPre = lib.mkIf (hasPlasmaManager && cfg.autoPaste) "-${pkgs.kdePackages.kservice}/bin/kbuildsycoca6";
          };
        Install.WantedBy = ["graphical-session.target"];
      };
    }

    (lib.mkIf (cfg.disableKlipperHistory && hasPlasmaManager) {
      # Klipper otherwise records every copy too (persisting it unencrypted
      # with KeepClipboardContents) and races spoold to re-publish the
      # selection when its owner exits.
      programs.plasma.configFile.klipperrc.General = {
        KeepClipboardContents = false;
        PreventEmptyClipboard = false;
        IgnoreSelection = true;
        MaxClipItems = 1;
      };
    })
    (lib.mkIf cfg.disableKlipperShortcut (
      if hasPlasmaManager
      then {
        # mkForce: home-manager/bcnelson/_mixins/kde.nix binds this action to
        # Meta+V explicitly; with Spool enabled that must lose.
        programs.plasma.shortcuts.${klipperGroup}.${klipperAction} = lib.mkForce [];
      }
      else {
        # No plasma-manager: edit kglobalshortcutsrc directly, only where a KDE
        # session has already created it (so non-KDE hosts are untouched).
        home.activation.spoolDisableKlipperShortcut = lib.hm.dag.entryAfter ["writeBoundary"] ''
          f="''${XDG_CONFIG_HOME:-$HOME/.config}/kglobalshortcutsrc"
          if [ -f "$f" ]; then
            run ${pkgs.kdePackages.kconfig}/bin/kwriteconfig6 --file "$f" \
              --group ${klipperGroup} --key ${klipperAction} \
              "none,Meta+V,Show Clipboard Items at Mouse Position"
          fi
        '';
      }
    ))
  ]);
}
