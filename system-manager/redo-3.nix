{ lib, ... }:
{
  config = {
    system-manager.allowAnyDistro = true;

    # The same source file romeo and sierra use, so all three decrypt to the
    # identical value -- agenix-rekey re-encrypts it per host, keyed to the host
    # pubkey already registered in ../hosts/data/redo-3.nix (verified to match
    # this machine's /etc/ssh/ssh_host_ed25519_key.pub).
    #
    # This replaces a file copied here by hand. That copy was invisible to
    # `just rekey`: regenerating the secret on romeo -- it has a
    # `generator.script` -- left redo-3 holding a stale value with no signal
    # beyond goose-desktop failing to connect. Sharing the rekeyFile makes the
    # two move together.
    #
    # owner is set because goose-desktop runs as bcnelson and reads the key at
    # launch. See ../home-manager/bcnelson/_mixins/programs/goose-desktop.nix.
    age.secrets.goose-server-secret-key = {
      rekeyFile = ../nixos/romeo/services/secrets/goose_server_secret_key.age;
      owner = "bcnelson";
      mode = "0400";
    };

    # bcnelson already exists on the host with matching UID/GID/groups, so
    # userborn would only churn /etc/passwd for no benefit and races with the
    # home-manager activation service that depends on a stable User= lookup.
    services.userborn.enable = false;

    # system-manager unconditionally emits `After=userborn.service` in its
    # generated suid-sgid-wrappers.service, and its engine unconditionally
    # restarts userborn.service before tmpfiles. With userborn disabled the unit
    # doesn't exist, so systemd pins a not-found stub (the dangling After keeps
    # it referenced, surviving daemon-reload) and every activation logs
    # "Unit userborn.service not found". Provide a real no-op oneshot: it
    # satisfies the ordering and the restart probe without touching /etc/passwd.
    environment.etc."systemd/system/userborn.service" = {
      mode = "0644";
      text = ''
        [Unit]
        Description=No-op userborn (userborn disabled on redo-3)
        [Service]
        Type=oneshot
        RemainAfterExit=yes
        ExecStart=/usr/bin/true
      '';
    };

    users.groups.bcnelson.gid = 1000;
    users.users.bcnelson = {
      isNormalUser = true;
      uid = 1000;
      group = "bcnelson";
      home = "/home/bcnelson";
      createHome = false;
    };

    home-manager.backupFileExtension = "bak";

    # Redo's local dev stack (`don`) runs a Caddy reverse proxy as bcnelson,
    # serving https://*.getredo.localhost on :443 with a :80 HTTP redirect. It
    # died on every start with
    #   listening on :443: listen tcp :443: bind: permission denied
    # because Fedora's ip_unprivileged_port_start is 1024 and this host's caddy
    # comes from the Nix profile, which -- unlike the Debian package everyone
    # else on the team installs -- carries no cap_net_bind_service.
    #
    # Lowering the threshold rather than setting the capability: the binary
    # lives at a content-addressed /nix/store path, so a setcap would be lost on
    # the next caddy update or store GC, and system-manager has no way to
    # reapply it.
    system.sysctl."net.ipv4.ip_unprivileged_port_start" = 80;

    # Bluetooth audio silently died on 2026-09-11 when a resume from suspend
    # landed on top of a global OOM kill (a Firefox content process at 11.8G
    # RSS). bluetoothd stalled long enough for wireplumber's bluez5 monitor to
    # hit its 25s D-Bus timeout --
    #   spa.bluez5: GetManagedObjects() failed: org.freedesktop.DBus.Error.NoReply
    # -- and that monitor has no retry path. It unregistered every
    # /MediaEndpoint/A2DP{Sink,Source}/* from bluetoothd and never registered
    # them again. Headphones still paired and connected, because HID and AVRCP
    # live in bluetoothd rather than PipeWire, while every audio attempt got
    #   a2dp-sink profile connect failed: Protocol not available
    # and no sink ever appeared. That asymmetry is why it went five days
    # unnoticed; only `systemctl --user restart wireplumber` clears it.
    #
    # Nothing declarative can make that monitor retry, so the lever here is the
    # memory pressure that stalled bluetoothd to begin with. Fedora's
    # zram-generator-defaults ships `zram-size = min(ram, 8192)`, i.e. 8G on
    # this 60.6G machine, and it sat pinned at 8.0G of 8.0G used through nine
    # OOM kills in the preceding thirty days.
    #
    # ram/2 is ~30.3G here. zram was compressing 7.7G of pages down to 2.6G, so
    # at that ~3:1 ratio a fully saturated 30G device costs ~10G of real RAM --
    # affordable out of 60.6G, and enough headroom that the thrashing which
    # preceded the stall should stop happening.
    #
    # This file overrides /usr/lib/systemd/zram-generator.conf, so it also
    # survives the dnf updates that rewrite the shipped defaults. The new size
    # applies at the next boot: resizing a live zram device means swapoff first,
    # which would fault the entire backing set back into RAM.
    environment.etc."systemd/zram-generator.conf" = {
      mode = "0644";
      replaceExisting = true;
      text = ''
        # Managed by system-manager -- see ./redo-3.nix
        [zram0]
        zram-size = ram / 2
        compression-algorithm = zstd
      '';
    };

    # Fedora's default of 60 is tuned for swapping to a disk. With zram the swap
    # device *is* RAM, so a compression round-trip is far cheaper than evicting
    # page cache and the kernel should reach for it much sooner -- the Arch wiki
    # recommends going beyond 100 for in-memory swap, and 180 is what Pop!_OS
    # ships. Without this the enlarged device would stay mostly unused under the
    # same pressure that caused the stall.
    system.sysctl."vm.swappiness" = 180;

    # On 2026-10-12 Ghostty kept opening windows with no shell in them, logging
    #   failed to initialize surface err=error.SystemResources
    # for each one. That is io_uring_setup(2) returning ENOMEM: an unprivileged
    # ring's memory is charged to the *user's* locked-page count and checked
    # against RLIMIT_MEMLOCK, which systemd leaves at 8M. Every Node process
    # holds three rings through libuv, and 18 Claude Code sessions with their
    # MCP servers added up to ~95 of those -- 284 of the 289 rings on the box,
    # right at the 8M ceiling. Ghostty needs a fresh ring per surface and has
    # no fallback once the app is already running on io_uring.
    #
    # The limit is per user, so the terminal pays for whatever else is running.
    # 256M is ~30x the load that hit the ceiling and still a hard bound on
    # pinned memory. Set on user@.service because everything in the graphical
    # session inherits its rlimits from the user manager; it applies the next
    # time that manager starts (logout or reboot).
    environment.etc."systemd/system/user@.service.d/10-memlock.conf" = {
      mode = "0644";
      text = ''
        [Service]
        LimitMEMLOCK=256M
      '';
    };

    # Upstream system-manager writes this file with systemd's syntax --
    #   PATH=/etc/profiles/per-user/${USER}/bin:/run/system-manager/sw/bin:${PATH}
    # -- which systemd expands, but under sudo root ends up with that literal,
    # unexpanded string as its PATH (most likely via pam_env, which Fedora 43
    # builds against libeconf). Shell scriptlets in rpm get their own PATH, so
    # the only visible symptom was glibc-common's Lua file trigger failing to
    # find `ldconfig` and every `sudo dnf` transaction ending in
    #   Transaction failed: Rpm transaction failed.
    #
    # Values with no variable references read the same in both parsers. This
    # drops the /etc/profiles/per-user entries (that directory does not exist
    # here) and NIX_PROFILES / NIX_USER_PROFILE_DIR.
    environment.etc."environment.d/10-system-manager.conf".text = lib.mkForce ''
      PATH=/run/system-manager/sw/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin
      XDG_DATA_DIRS=/run/system-manager/sw/share:/usr/local/share:/usr/share
    '';

    # Disable USB autosuspend on the Goodix 27c6:609c fingerprint reader so
    # it stays responsive while the lock screen is up.
    environment.etc."udev/rules.d/60-goodix-fingerprint-no-suspend.rules" = {
      mode = "0644";
      replaceExisting = true;
      text = ''
        # Goodix 27c6:609c — keep fingerprint reader awake so unlock prompt stays responsive
        ACTION=="add", SUBSYSTEM=="usb", ATTR{idVendor}=="27c6", ATTR{idProduct}=="609c", TEST=="power/control", ATTR{power/control}="on"
      '';
    };

    # Keychron V6 (3434:0361) on the dock. Fedora ships no QMK udev rules, so
    # without this the board's /dev/hidraw* stay root:root 0600 and VIA cannot
    # open them.
    hardware.keyboard.qmk.enable = true;

    # voxtype types its transcript with dotool, which writes to /dev/uinput.
    hardware.uinput = {
      enable = true;
      users = [ "bcnelson" ];
    };

    # Strip Fedora's `--no-timeout` from fprintd. The Goodix reader's libusb
    # state gets stuck after a system suspend/resume; if fprintd stays
    # running, every later kscreenlocker auth silently fails on a wedged
    # Claim. Restoring the default idle-shutdown makes fprintd exit ~30s
    # after the last DBus call, so the next pam_fprintd activates a fresh
    # daemon process with a fresh USB context.
    environment.etc."systemd/system/fprintd.service.d/override.conf" = {
      mode = "0644";
      replaceExisting = true;
      text = ''
        [Service]
        ExecStart=
        ExecStart=/usr/libexec/fprintd
      '';
    };

    # Override plasma-workspace's /etc/pam.d/kde-fingerprint to bump the
    # lock-screen fingerprint timeout from 600s to 2h. Copied (not symlinked)
    # so PAM reads a real file; SELinux relabel is handled generically by
    # system-manager/_mixins/selinux.nix.
    environment.etc."pam.d/kde-fingerprint" = {
      mode = "0644";
      replaceExisting = true;
      text = ''
        auth    required    pam_env.so
        auth    [success=done default=bad]  pam_fprintd.so timeout=7200 max-tries=10
        auth    required    pam_deny.so

        auth        include       postlogin

        account     required      pam_nologin.so
        account     include       fingerprint-auth

        password    include       fingerprint-auth

        session     required      pam_selinux.so close
        session     required      pam_loginuid.so
        session     required      pam_selinux.so open
        session     optional      pam_keyinit.so force revoke
        session     required      pam_namespace.so
        session     include       fingerprint-auth
        session     include       postlogin
      '';
    };
  };
}
