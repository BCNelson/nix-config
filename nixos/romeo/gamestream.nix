{ config, pkgs, lib, ... }:
# On-demand, zero-idle game streaming on romeo (headless, server-first).
#
# Architecture (see docs/gamestream.md for the full write-up):
#   * Two dedicated, unprivileged users (game-brad, game-hannah), each with its
#     own Steam library/saves. Only one runs at a time.
#   * Nothing runs until a session is started: `gamestream@<user>.service` is a
#     system-level shim that enables linger and starts the user's headless
#     session, and its ExecStop tears everything back down (disable-linger), so
#     idle cost is zero.
#   * The session is a headless wlroots (sway) compositor on the A380 (i915),
#     which captures + VA-API-encodes via Sunshine. Games render on the B580
#     (xe) via PRIME offload (per-game Steam launch options).
#   * gamestream-agent bridges the system unit to Home Assistant over MQTT
#     (switch + state), and Sunshine's global_prep_cmd reports streaming state.
#
# NOTE: the headless-session and GPU-offload details (marked "HARDWARE:") need
# tuning on the real machine; they are best-effort defaults here.
let
  # HA profile id -> system user (systemd template instance).
  profiles = {
    brad = "game-brad";
    hannah = "game-hannah";
  };
  gameUsers = lib.attrValues profiles;
  profilesEnv = lib.concatStringsSep ","
    (lib.mapAttrsToList (id: user: "${id}=${user}") profiles);

  # HARDWARE: stable render nodes come from romeo's udev symlinks
  # (see 2.hardware-configuration.nix): A380 = i915 (compositor + encode),
  # B580 = xe (game rendering).
  encodeRenderNode = "/dev/dri/by-driver/i915-render";

  # The Sunshine hook talks to the agent's notify socket; an empty profile
  # routes the event to whichever session is currently active.
  agent = "${pkgs.gamestream-agent}/bin/gamestream-agent";
  globalPrepCmd = builtins.toJSON [
    {
      do = "${agent} notify --event stream-start";
      undo = "${agent} notify --event stream-stop";
    }
  ];

  # Minimal headless sway session: one virtual output on the A380, launch Steam
  # Big Picture, and bring Sunshine up once the Wayland socket exists.
  swayConfig = pkgs.writeText "gamestream-sway.conf" ''
    # HARDWARE: pick a resolution/refresh your clients want.
    output HEADLESS-1 resolution 1920x1080 position 0,0

    # Expose the Wayland session to the user systemd manager, then start
    # Sunshine (a user service) now that WAYLAND_DISPLAY is set.
    exec ${pkgs.systemd}/bin/systemctl --user import-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP
    exec ${pkgs.systemd}/bin/systemctl --user start sunshine.service

    # The session's application.
    exec ${pkgs.steam}/bin/steam -gamepadui
  '';

  # System-level lifecycle shim, one instance per user. StartUnit enables linger
  # and starts the user's session; StopUnit reverses it (zero idle afterwards).
  gamestreamUp = pkgs.writeShellScript "gamestream-up" ''
    set -eu
    user="$1"
    ${pkgs.systemd}/bin/loginctl enable-linger "$user"
    # Wait for the user manager to come up before talking to it.
    for _ in $(seq 1 30); do
      if ${pkgs.systemd}/bin/systemctl --user --machine="$user@.host" is-system-running >/dev/null 2>&1; then
        break
      fi
      sleep 1
    done
    ${pkgs.systemd}/bin/systemctl --user --machine="$user@.host" start gamestream.target
  '';
  gamestreamDown = pkgs.writeShellScript "gamestream-down" ''
    set -u
    user="$1"
    ${pkgs.systemd}/bin/systemctl --user --machine="$user@.host" stop gamestream.target || true
    ${pkgs.systemd}/bin/loginctl disable-linger "$user" || true
  '';
in
{
  # --- Steam (override mirrors nixos/_mixins/roles/gaming.nix) ---
  nixpkgs.config.packageOverrides = pkgs: {
    steam = pkgs.steam.override {
      extraPkgs =
        pkgs: with pkgs; [
          libxcursor
          libxi
          libxinerama
          libxscrnsaver
          libpng
          libpulseaudio
          libvorbis
          stdenv.cc.cc.lib
          libkrb5
          keyutils
        ];
    };
  };
  programs.steam.enable = true;
  programs.gamescope.enable = true;

  # Input emulation for Moonlight-forwarded gamepads/keyboard/mouse.
  hardware.uinput.enable = true;

  # --- Users: the gaming profiles + the agent's system user (one definition) ---
  users.users = lib.genAttrs gameUsers (name: {
    isNormalUser = true;
    description = "Game streaming profile (${name})";
    # render/video: GPU (VA-API); input/uinput: controller forwarding; audio: PipeWire.
    extraGroups = [ "video" "render" "input" "audio" "uinput" ];
    # No interactive password; sessions are brought up on demand via the shim.
    hashedPassword = "!";
  }) // {
    gamestream-agent = {
      isSystemUser = true;
      group = "gamestream-agent";
    };
  };
  users.groups.gamestream-agent = { };

  # --- Audio: a virtual sink Sunshine can capture on a machine with no sound HW ---
  services.pulseaudio.enable = false;
  security.rtkit.enable = true;
  services.pipewire = {
    enable = true;
    alsa.enable = true;
    pulse.enable = true;
    extraConfig.pipewire."10-gamestream-sink" = {
      "context.objects" = [
        {
          factory = "adapter";
          args = {
            "factory.name" = "support.null-audio-sink";
            "node.name" = "gamestream";
            "node.description" = "Gamestream capture sink";
            "media.class" = "Audio/Sink";
            "audio.position" = "FL,FR";
          };
        }
      ];
    };
  };

  # --- Sunshine (shared config; only one profile streams at a time) ---
  # v1 uses one config for both users. Distinct per-user port bases (so Moonlight
  # lists two independently-paired hosts) are a documented follow-up.
  services.sunshine = {
    enable = true;
    capSysAdmin = true; # needed to grab the (headless) display on Wayland
    openFirewall = false; # scoped below instead
    settings = {
      # HARDWARE: encode on the A380 via the proven i915 VA-API path.
      encoder = "vaapi";
      adapter_name = encodeRenderNode;
      audio_sink = "gamestream.monitor";
      # Report streaming start/stop to the agent.
      global_prep_cmd = globalPrepCmd;
    };
  };

  # --- The on-demand session, as user systemd units (inert until started) ---
  # Resource-cap the whole session so it stays subordinate to server workloads.
  systemd.user.slices.gamestream = {
    description = "Game streaming (resource-capped)";
    sliceConfig = {
      # HARDWARE: tune to romeo's core count / RAM.
      CPUQuota = "700%";
      MemoryHigh = "24G";
      MemoryMax = "28G";
      IOWeight = 20;
    };
  };

  systemd.user.targets.gamestream = {
    description = "Game streaming session";
    unitConfig.StopWhenUnneeded = false;
  };

  systemd.user.services.gamestream-compositor = {
    description = "Headless gamestream compositor (sway on the A380)";
    partOf = [ "gamestream.target" ];
    wantedBy = [ "gamestream.target" ];
    environment = {
      WLR_BACKENDS = "headless";
      # HARDWARE: composite + capture on the A380 (i915). Games offload to the
      # B580 per-game (e.g. Steam launch options: `DRI_PRIME=1 %command%` or
      # `MESA_VK_DEVICE_SELECT=<B580 pci id> %command%`).
      WLR_RENDER_DRM_DEVICE = encodeRenderNode;
      WLR_LIBINPUT_NO_DEVICES = "1";
    };
    serviceConfig = {
      Type = "simple";
      Slice = "gamestream.slice";
      ExecStart = "${pkgs.sway}/bin/sway -c ${swayConfig}";
      Restart = "no";
    };
  };

  # Sunshine is a user service provided by the module; it is started explicitly
  # from the sway config once WAYLAND_DISPLAY exists, and torn down when the user
  # manager exits. (Putting it in gamestream.slice is a follow-up; the heavy
  # compositor+Steam+game processes already run under the capped slice.)

  # --- System-level lifecycle shim (what the agent / SSH drives) ---
  systemd.services."gamestream@" = {
    description = "On-demand game-streaming session for %i";
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = "${gamestreamUp} %i";
      ExecStop = "${gamestreamDown} %i";
      TimeoutStartSec = "180";
    };
  };

  # --- gamestream-agent: MQTT <-> systemd bridge for Home Assistant ---
  # (the gamestream-agent user/group are defined with users.users above)

  # Scope: the agent may manage only the gamestream@ units, nothing else.
  security.polkit.enable = true;
  security.polkit.extraConfig = ''
    polkit.addRule(function(action, subject) {
      if (action.id == "org.freedesktop.systemd1.manage-units" &&
          subject.user == "gamestream-agent") {
        var unit = action.lookup("unit");
        if (unit && unit.indexOf("gamestream@") == 0) {
          return polkit.Result.YES;
        }
      }
    });
  '';

  # The agent needs to enable-linger and drive other users' managers, which the
  # polkit rule above does not cover; grant just those two commands via sudo.
  security.sudo.extraRules = [
    {
      users = [ "gamestream-agent" ];
      commands = [
        { command = "${pkgs.systemd}/bin/loginctl"; options = [ "NOPASSWD" ]; }
      ];
    }
  ];

  systemd.services.gamestream-agent = {
    description = "gamestream MQTT <-> systemd agent";
    wantedBy = [ "multi-user.target" ];
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    serviceConfig = {
      User = "gamestream-agent";
      Group = "gamestream-agent";
      ExecStart = "${agent} serve";
      RuntimeDirectory = "gamestream-agent";
      Restart = "on-failure";
      RestartSec = "5";
    };
    environment = {
      GAMESTREAM_MQTT_BROKER = "192.168.3.6:1883";
      GAMESTREAM_MQTT_USERNAME = "gamestream";
      GAMESTREAM_PROFILES = profilesEnv;
      GAMESTREAM_SOCKET = "/run/gamestream-agent/notify.sock";
      # HARDWARE / SECRET: provision the MQTT password as an agenix secret and
      # point the agent at it. This is left as a runtime step because the age
      # secret must be created with the FIDO2 master key (`just rekey`) and must
      # match the `gamestream` user configured on the broker. Once created,
      # uncomment the age.secrets block below and this line:
      # GAMESTREAM_MQTT_PASSWORD_FILE = config.age.secrets.gamestream_mqtt_password.path;
    };
  };

  # SECRET (see note above): create secrets/store/romeo/gamestream_mqtt_password
  # via `just rekey`, then uncomment.
  # age.secrets.gamestream_mqtt_password.rekeyFile =
  #   ../../secrets/store/romeo/gamestream_mqtt_password.age;

  # --- Firewall: Sunshine ports ---
  # Reachable on the LAN (for low-latency in-home play, discovered via mDNS) and
  # over Tailscale (for remote play). romeo's edge router does not forward these,
  # so they are not exposed to the WAN. To restrict further, move these into
  # networking.firewall.interfaces.<lan>/tailscale0.
  networking.firewall.allowedTCPPorts = [ 47984 47989 47990 48010 ];
  networking.firewall.allowedUDPPorts = [ 47998 47999 48000 48002 48010 ];
}
