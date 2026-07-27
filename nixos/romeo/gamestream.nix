{ config, pkgs, lib, ... }:
# On-demand, zero-idle game streaming on romeo (headless, server-first).
#
# Architecture (see docs/gamestream.md for the full write-up):
#   * Three dedicated, unprivileged users (game-bcnelson, game-hlnelson,
#     game-family), each with its own Steam library/saves. Only one runs at a
#     time.
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
  # HA profile id -> system user (systemd template instance). Profile ids match
  # the human usernames used elsewhere in this repo; "family" is a shared
  # profile (couch/guest play) with its own Steam library.
  profiles = {
    bcnelson = "game-bcnelson";
    hlnelson = "game-hlnelson";
    family = "game-family";
  };
  gameUsers = lib.attrValues profiles;
  profilesEnv = lib.concatStringsSep ","
    (lib.mapAttrsToList (id: user: "${id}=${user}") profiles);

  # HARDWARE: stable render nodes come from romeo's udev symlinks
  # (see 2.hardware-configuration.nix): A380 = i915 (compositor + encode),
  # B580 = xe (game rendering).
  encodeRenderNode = "/dev/dri/by-driver/i915-render";

  # Sunshine's CSRF check only trusts localhost variants out of the box, so the
  # web UI rejects every request ("CSRF Protection Error") when it is reached by
  # IP or DNS name -- which is the only way in on a headless box. Origins are a
  # comma-separated list and must carry protocol, host and port.
  webUiHost = "gamestream.h.b.nel.family";
  webUiOrigins = [
    "https://${webUiHost}" # nginx vhost (LAN + tailnet), port 443 so no :port
    "https://192.168.3.7:47990" # LAN address, direct
    "https://romeo.b.nel.family:47990" # LAN DNS, direct
    "https://100.76.49.168:47990" # tailscale, direct
  ];

  # Sunshine has no declarative credential option (the NixOS module exposes
  # none, and the salted hash in sunshine_state.json is generated with a random
  # salt, so it cannot be rendered from Nix). `sunshine --creds` is the only
  # supported way in, and it preserves the paired-device list in that same file,
  # so running it on every start is safe and lets a password change actually
  # take effect.
  #
  # The web-UI username is the profile id (game-bcnelson -> bcnelson), matching
  # the Home Assistant entity names, and each profile has its own passphrase.
  #
  # This unit is defined for every user on the host, so anything that is not a
  # game profile falls through and leaves Sunshine's credentials alone rather
  # than failing to start it.
  sunshineSetCreds = pkgs.writeShellScript "sunshine-set-creds" ''
    set -eu
    user="$(${pkgs.coreutils}/bin/id -un)"
    case "$user" in
      ${lib.concatStringsSep "\n      " (lib.mapAttrsToList
        (id: u: ''${u}) profile='${id}'; secret='${config.age.secrets."sunshine_web_password_${id}".path}';;'')
        profiles)}
      *)
        echo "sunshine-set-creds: $user is not a gamestream profile, skipping"
        exit 0
        ;;
    esac
    if [ ! -r "$secret" ]; then
      echo "sunshine-set-creds: $secret not readable, leaving credentials alone"
      exit 0
    fi
    exec ${config.services.sunshine.package}/bin/sunshine \
      --creds "$profile" "$(${pkgs.coreutils}/bin/cat "$secret")"
  '';

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
    # Declare an explicit refresh rate. Without one wlroots reports the output
    # as 0 Hz, and Moonlight happily asks for whatever it likes (it requested
    # 165 fps, against which Sunshine set a "minimum FPS target" of ~82) -- so
    # capture pacing had no upper bound to work against. 60 Hz matches what this
    # box can actually feed a 1080p stream.
    output HEADLESS-1 resolution 1920x1080@60Hz position 0,0

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

  # Seat provider for the headless session's libinput backend (see the
  # compositor's LIBSEAT_BACKEND above). romeo has no graphical login, so
  # nothing else would start one.
  services.seatd.enable = true;

  # --- Users: the gaming profiles + the agent's system user (one definition) ---
  users.users = lib.genAttrs gameUsers (name: {
    isNormalUser = true;
    description = "Game streaming profile (${name})";
    # render/video: GPU (VA-API); input/uinput: controller forwarding; audio:
    # PipeWire; seat: libseat access so the compositor can open input devices.
    extraGroups = [ "video" "render" "input" "audio" "uinput" "seat" ];
    # No interactive password; sessions are brought up on demand via the shim.
    hashedPassword = "!";
  }) // {
    gamestream-agent = {
      isSystemUser = true;
      group = "gamestream-agent";
    };
  };
  users.groups.gamestream-agent = { };

  # One generated passphrase per profile, each owned by that profile's user so
  # it can read its own secret (agenix defaults to root-only, which an
  # unprivileged session cannot use), and each synced to Bitwarden so the human
  # who has to type it into the Sunshine web UI can find it.
  age.secrets = {
    # Must match the `gamestream` user on the broker. The agent reads this path
    # as its own unprivileged user, so it needs an owner (agenix defaults to
    # root 0400).
    gamestream_mqtt_password = {
      rekeyFile = ../../secrets/store/romeo/gamestream_mqtt_password.age;
      owner = "gamestream-agent";
    };
  } // lib.mapAttrs'
    (id: user: lib.nameValuePair "sunshine_web_password_${id}" {
      rekeyFile = ../../secrets/store/romeo/sunshine_web_password_${id}.age;
      generator.script = "passphrase";
      owner = user;
      mode = "0400";
      bitwarden = {
        name = "Sunshine (romeo) - ${id}";
        username = id;
        uris = [
          { uri = "https://${webUiHost}"; matchType = "host"; }
          { uri = "https://192.168.3.7:47990"; matchType = "host"; }
        ];
      };
    })
    profiles;

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

  # The module's capSysAdmin only grants cap_sys_admin, but Sunshine also wants
  # cap_sys_nice to raise its capture/encode thread priority and to request a
  # high-priority EGL context. Without it the log fills with "setpriority failed
  # for nice -10/-15: Permission denied" and "EGL: context priority set to HIGH
  # but CAP_SYS_NICE capability is missing", and frame pacing is at the mercy of
  # whatever else romeo is doing (Ollama, Frigate, the *arrs).
  security.wrappers.sunshine.capabilities = lib.mkForce "cap_sys_admin,cap_sys_nice+p";

  # --- Sunshine (shared config; only one profile streams at a time) ---
  # v1 uses one config for all profiles. Distinct per-user port bases (so
  # Moonlight lists independently-paired hosts) are a documented follow-up.
  services.sunshine = {
    enable = true;
    capSysAdmin = true; # needed to grab the (headless) display on Wayland
    openFirewall = false; # scoped below instead
    settings = {
      # HARDWARE: encode on the A380 via the proven i915 VA-API path.
      encoder = "vaapi";
      adapter_name = encodeRenderNode;
      # Loopback capture source: the null sink's monitor.
      audio_sink = "gamestream.monitor";
      # Sunshine picks a virtual sink automatically when this is unset, and on a
      # headless box it picks "sink-sunshine-stereo" (Steam Streaming Speakers),
      # which does not exist here. It then makes that the default sink, resolves
      # its monitor to an empty name and dies with
      # "pa_simple_new() failed: Invalid argument -- The stream will not have
      # audio". Point it at our own null sink instead. Upstream recommends
      # leaving this blank, but that advice assumes a desktop with real devices.
      virtual_sink = "gamestream";
      # Without these the web UI is unusable from anywhere but localhost.
      csrf_allowed_origins = lib.concatStringsSep "," webUiOrigins;
      # Report streaming start/stop to the agent.
      global_prep_cmd = globalPrepCmd;
    };
  };

  # --- The on-demand session, as user systemd units (inert until started) ---
  # Resource-cap the whole session so it stays subordinate to server workloads.
  systemd.user.slices.gamestream = {
    description = "Game streaming (resource-capped)";
    sliceConfig = {
      # Stay subordinate to the server workloads by *weight*, not by a hard cap.
      # frigate/ollama/jellyfin/postgresql all run at the default weight of 100,
      # so 50 means the session yields to them under contention while still
      # being free to use an otherwise-idle machine. (system-autoupdate.slice
      # uses 10 for the same reason; a live game session should outrank a
      # background rebuild.)
      CPUWeight = 50;
      # romeo has 32 cores. The previous 700% was a placeholder written before
      # anyone measured the box: it capped the session at 22% of the machine
      # *even when idle*, and throttled 1351 of 6552 periods (~2679s) in a
      # single evening -- most painfully during shader compilation, which is
      # exactly when a game wants every core it can get. Keep a quota purely as
      # a runaway backstop, leaving 8 cores' worth of headroom regardless.
      CPUQuota = "2400%";
      # HARDWARE: 94G total on romeo, so there is room to raise these if a game
      # ever pushes into them.
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
    # sway runs every `exec` line through `sh -c`, so it needs a shell on PATH.
    # The default unit PATH (coreutils/findutils/gnugrep/gnused/systemd) has
    # none, which made all three execs in swayConfig die with
    # "execve failed: No such file or directory" -- sway itself came up fine,
    # but nothing it was supposed to launch (Sunshine included) ever ran.
    path = [ pkgs.bash ];
    environment = {
      # "headless" alone gives a virtual output but builds NO libinput backend,
      # so wlroots never enumerates input devices: `swaymsg -t get_inputs`
      # returns [] and Moonlight's keyboard/mouse/gamepad go nowhere. Sunshine
      # injects input by creating uinput devices, and something has to read
      # them back -- that is the libinput backend.
      WLR_BACKENDS = "headless,libinput";
      # wlroots opens input devices through libseat. This session is a lingering
      # `systemctl --user` manager with no logind seat (that is inherent to the
      # zero-idle design), so libseat's logind backend cannot work; seatd
      # provides a seat instead. See services.seatd below.
      LIBSEAT_BACKEND = "seatd";
      # HARDWARE: composite + capture on the A380 (i915). Games offload to the
      # B580 per-game (e.g. Steam launch options: `DRI_PRIME=1 %command%` or
      # `MESA_VK_DEVICE_SELECT=<B580 pci id> %command%`).
      WLR_RENDER_DRM_DEVICE = encodeRenderNode;
      # Start even when no input devices are present yet; Sunshine's uinput
      # devices only appear once a client connects.
      WLR_LIBINPUT_NO_DEVICES = "1";
    };
    serviceConfig = {
      Type = "simple";
      Slice = "gamestream.slice";
      ExecStart = "${pkgs.sway}/bin/sway -c ${swayConfig}";
      Restart = "no";
    };
  };

  # Set the web-UI credentials before Sunshine starts (see sunshineSetCreds).
  systemd.user.services.sunshine.serviceConfig.ExecStartPre = [ "${sunshineSetCreds}" ];

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
      GAMESTREAM_MQTT_PASSWORD_FILE = config.age.secrets.gamestream_mqtt_password.path;
    };
  };

  # --- Web UI behind nginx (LAN + tailnet only) ---
  # Sunshine's own listener is HTTPS with a self-signed certificate, which means
  # a browser warning on every visit and no usable name. Front it with a real
  # ACME cert on a proper hostname, restricted the same way ai.h.b.nel.family
  # is: reachable over Tailscale and from the LAN, denied everywhere else. The
  # 47990 port stays open for direct access, and both are in
  # csrf_allowed_origins above.
  #
  # proxy_ssl_verify is off by default, which is what we want here -- the
  # upstream certificate is Sunshine's self-signed one and cannot be verified.
  services.nginx.virtualHosts.${webUiHost} = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      # Allow access from Tailscale network
      allow 100.64.0.0/10;
      # Allow access from local network
      allow 192.168.0.0/16;
      deny all;
    '';
    locations."/" = {
      proxyPass = "https://127.0.0.1:47990";
      proxyWebsockets = true;
    };
  };

  # --- Firewall: Sunshine ports ---
  # Reachable on the LAN (for low-latency in-home play, discovered via mDNS) and
  # over Tailscale (for remote play). romeo's edge router does not forward these,
  # so they are not exposed to the WAN. To restrict further, move these into
  # networking.firewall.interfaces.<lan>/tailscale0.
  networking.firewall.allowedTCPPorts = [ 47984 47989 47990 48010 ];
  networking.firewall.allowedUDPPorts = [ 47998 47999 48000 48002 48010 ];
}
