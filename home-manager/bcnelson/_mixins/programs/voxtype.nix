{ lib, pkgs, ... }:
let
  # -vulkan offloads whisper to the Radeon. The model lives in the store rather
  # than being pulled into ~/.local/share by `voxtype setup --download`, so a
  # rebuild is reproducible and the daemon never needs the network.
  package = pkgs.voxtype-vulkan;

  model = pkgs.fetchurl {
    url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin";
    hash = "sha256-OUIhcJzVrR9AxG5gMcphvOiJMebgiMGIKUxtWlX/p+I=";
  };

  settings = {
    state_file = "auto";

    # Everything is driven by the KDE shortcuts below, via `voxtype record`.
    #
    # voxtype's own evdev listener is deliberately off. It is the only way to get
    # hold-to-talk (KDE shortcuts fire on press, so there is no release event to
    # end a hold), but its `modifiers` list is a "these must ALSO be held" check
    # with no way to require that nothing else is held. Measured on this machine:
    # with modifiers = [], Shift+<key> starts a recording just like a bare <key>
    # does. Since the listener also never EVIOCGRABs, KDE sees the same
    # press, so Shift+mic would start a push-to-talk recording AND fire the KDE
    # toggle -- two racing commands to one daemon. Toggle on a single key avoids
    # the whole problem, and costs no privileges: no input group, no /dev/input.
    hotkey.enabled = false;

    audio = {
      device = "default";
      sample_rate = 16000;
      max_duration_secs = 120;
    };

    whisper = {
      mode = "local";
      model = "${model}";
      language = "en";
      # Load on record-start and unload at idle. The load takes ~0.3s and
      # happens while you are still speaking, so it costs nothing perceptible
      # and keeps 573MB of VRAM free for games and ollama between dictations.
      on_demand_loading = true;
    };

    output = {
      mode = "type";
      # KWin does not implement zwp_virtual_keyboard_manager_v1, so wtype always
      # fails here ("Compositor does not support the virtual keyboard protocol").
      # dotool writes to /dev/uinput directly and needs no daemon.
      driver_order = [ "dotool" "clipboard" ];
      fallback_to_clipboard = true;
      type_delay_ms = 0;
      notification = {
        on_recording_start = true;
        on_recording_stop = true;
        on_transcription = true;
      };
    };

    # The waveform overlay wants a Quickshell tree installed at runtime; the
    # notifications above already say when it is listening.
    osd.enabled = false;
  };

  configFile = (pkgs.formats.toml { }).generate "voxtype-config.toml" settings;
in
{
  home.packages = [ package ];

  # The daemon reads the store copy directly, but the `voxtype record` client
  # invoked by the hotkey runs outside the sandbox and looks here to find out
  # where the state file lives.
  xdg.configFile."voxtype/config.toml".source = configFile;

  # BindPaths= cannot create its own source, and voxtype opens a SQLite index
  # under this directory on startup, so it has to exist before the unit runs.
  home.file.".local/share/voxtype/.keep".text = "";

  systemd.user.services.voxtype = {
    Unit = {
      Description = "voxtype voice-to-text daemon";
      Documentation = "https://voxtype.io";
      PartOf = [ "graphical-session.target" ];
      After = [ "graphical-session.target" ];
      # Pick up config edits on the next `just update-home` instead of silently
      # running the old settings until logout.
      X-Restart-Triggers = [ "${configFile}" ];
    };

    Service = {
      # Read the config straight from the store rather than via ~/.config, so
      # the daemon does not depend on $HOME being reachable at all.
      ExecStart = "${lib.getExe package} -c ${configFile} daemon";
      # dotool is bundled in voxtype's wrapper; wl-clipboard backs the fallback.
      Environment = [ "PATH=${lib.makeBinPath [ package pkgs.wl-clipboard ]}" ];
      Restart = "on-failure";
      RestartSec = 5;

      # Hardening. Every directive below was verified on sierra by running a
      # full record -> GPU transcribe -> type cycle inside the sandbox; see the
      # notes on the omitted ones, which break it.
      NoNewPrivileges = true;
      ProtectSystem = "strict";

      # ProtectHome also masks /run/user, which is where the wayland, pipewire
      # and dbus sockets live, so %t has to be bound back in. What is left
      # hidden is the rest of $HOME: voxtype only sees its own data directory.
      ProtectHome = "tmpfs";
      BindPaths = [ "%t" "%h/.local/share/voxtype" ];
      BindReadOnlyPaths = [ "%h/.config/voxtype" ];

      PrivateTmp = true;
      ProtectProc = "invisible";
      ProcSubset = "pid";
      KeyringMode = "private";
      MemoryDenyWriteExecute = true;
      RestrictRealtime = true;
      ProtectKernelTunables = true;
      ProtectKernelModules = true;
      ProtectKernelLogs = true;
      ProtectControlGroups = true;
      ProtectClock = true;
      ProtectHostname = true;
      RestrictNamespaces = true;
      RestrictSUIDSGID = true;
      LockPersonality = true;
      SystemCallArchitectures = "native";

      # Adding ~@privileged or ~@resources on top of this kills the daemon with
      # SIGSYS the moment recording starts (pipewire and ggml need both sets).
      SystemCallFilter = "@system-service";

      # Transcription is entirely local and the model is in the store, so the
      # daemon has no business on the network at all. Cut off two ways: its own
      # netns with nothing but lo, and no socket family other than AF_UNIX (which
      # wayland, pipewire and dbus need, and which netns does not affect). This
      # is also what stops voxtype's GitHub release check from phoning home.
      #
      # IPAddressDeny is deliberately absent: unlike PrivateNetwork it is
      # silently ignored in a user unit ("unit configures an IP firewall, but not
      # running as root"), so it would look like protection without being any.
      PrivateNetwork = true;
      RestrictAddressFamilies = "AF_UNIX";

      # dotool needs /dev/uinput to synthesise keystrokes and whisper needs the
      # render node for Vulkan; nothing else in /dev is reachable. Notably absent
      # is char-input: with the evdev hotkey off, the daemon has no reason to
      # read the keyboards, so it cannot.
      DevicePolicy = "closed";
      DeviceAllow = [ "/dev/uinput rw" "char-drm rw" ];

      UMask = "0077";
    };

    Install.WantedBy = [ "graphical-session.target" ];
  };

  # The mic key is remapped to F14 in VIA. It ships as Meta+C (Cmd+C on the Mac
  # layer), which collides with applications and autorepeats "c" while held.
  #
  # "Launch (5)" is not a typo. XKB does not give the F13-F24 range F-key
  # keysyms -- evdev KEY_F14 comes through as XF86Launch5, whose Qt name is
  # "Launch (5)". So a binding written as "F14" is waiting for a keysym the
  # key never produces, and silently does nothing. The neighbours are no
  # better: KEY_F20 becomes XF86AudioMicMute, which KDE has already bound to
  # mute-microphone, and F13/F15-F18/F21/F22 land on XF86Tools, XF86Launch6-9
  # and the touchpad toggles. Only F19, F23 and F24 deliver the matching F-key
  # keysym, so those are the ones to move to if this ever needs to change.
  # (F23/F24 carry a second level on Shift+Super and Ctrl+Super respectively;
  # F19 is single-level and therefore the least surprising of the three.)
  #
  # Meta+V is already the clipboard applet, hence Meta+Alt+V for the
  # keyboard-independent fallback.
  programs.plasma.hotkeys.commands.voxtype-toggle = {
    name = "Voice input (toggle)";
    comment = "Start or stop voxtype dictation into the focused window";
    keys = [ "Launch (5)" "Meta+Alt+V" ];
    command = "${lib.getExe package} record toggle";
  };

  programs.plasma.hotkeys.commands.voxtype-cancel = {
    name = "Voice input (cancel)";
    comment = "Discard the current voxtype recording without typing it";
    keys = [ "Ctrl+Shift+Launch (5)" "Meta+Alt+Escape" ];
    command = "${lib.getExe package} record cancel";
  };
}
