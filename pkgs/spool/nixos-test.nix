# End-to-end VM test for Spool: a user with a lingering systemd user manager
# runs headless sway (pixman) as their graphical session, a throwaway unlocked
# gnome-keyring as the Secret Service, and spoold as a user service with
# exactly the unit the home-manager module ships (./systemd-service.nix).
#
# Exercises what the cargo E2E suite can't: the packaged binaries, the
# systemd sandbox in a *user* unit (including that D-Bus, Wayland and the
# Secret Service stay reachable from inside it), the encrypted store unlocked
# through a real Secret Service, persistence across restarts, and the PATH
# audit against a real unit environment.
#
# Run with: nix build .#spool-test -L   (needs KVM)
#
# Interactive: nix build .#spool-test.driverInteractive && ./result/bin/nixos-test-driver
{
  lib,
  testers,
  spool,
}: let
  unitDef = import ./systemd-service.nix {
    inherit lib;
    package = spool;
  };
  user = "alice";
  uid = 1000;
  runtime = "/run/user/${toString uid}";
  state = "/home/${user}/.local/state/spool";
  # `wl-copy` text that spool-core's policy recognises as a GitHub token
  # (`ghp_` + 36 alphanumerics; same fixture as crates/spoold/tests/e2e.rs).
  secret = "ghp_aB3dE6gH9jK2mN5pQ8sT1vW4yZ7bC0eF3hJ6";
in
  testers.runNixOSTest {
    name = "spool";

    nodes.machine = {pkgs, ...}: let
      swayConfig = pkgs.writeText "sway-headless.conf" ''
        output HEADLESS-1 resolution 1280x720
        # Publish the session to the user manager, then bring up the
        # graphical session (spoold is WantedBy graphical-session.target).
        exec ${pkgs.systemd}/bin/systemctl --user import-environment WAYLAND_DISPLAY SWAYSOCK && ${pkgs.systemd}/bin/systemctl --user start sway-session.target
      '';

      # key_provider = "session" (memory only), selected via SPOOL_CONFIG in a
      # drop-in for one subtest.
      sessionConfig = (pkgs.formats.toml {}).generate "spool-session.toml" {
        key_provider = "session";
      };

      # Runs with spoold's sandbox PLUS the namespace directives that were
      # left out of it (unitDef.namespaced), to keep that decision verified:
      # D-Bus and Wayland would still work, but /proc/<peer>/root (spoold's
      # peer check) becomes unreadable for a process outside the sandbox.
      probe = pkgs.writeShellScript "spool-sandbox-probe" ''
        set -u
        # sway runs outside the sandbox as the same uid, like spoolctl would.
        if stat -L /proc/"$(pgrep -u ${toString uid} -x sway | head -n1)"/root >/dev/null 2>&1; then
          peer=ok
        else
          peer=denied
        fi
        # %t itself is read-only in here; RuntimeDirectory=spool-probe is not.
        exec >${runtime}/spool-probe/out 2>&1
        echo "uid=$(id -u)"
        echo "peerroot=$peer"
        if busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus GetId >/dev/null; then
          echo "dbus=ok"
        else
          echo "dbus=fail"
        fi
        if busctl --user introspect org.freedesktop.secrets /org/freedesktop/secrets >/dev/null; then
          echo "secrets=ok"
        else
          echo "secrets=fail"
        fi
        if wayland-info >/dev/null 2>&1; then echo "wayland=ok"; else echo "wayland=fail"; fi
        if touch "$HOME/should-be-ro" 2>/dev/null; then echo "home=rw"; else echo "home=ro"; fi
        if touch ${runtime}/should-be-ro 2>/dev/null; then echo "runtime=rw"; else echo "runtime=ro"; fi
        if touch ${runtime}/spool-probe/x 2>/dev/null; then echo "rwpath=rw"; else echo "rwpath=ro"; fi
        # AF_INET is denied by RestrictAddressFamilies (socket() fails).
        if (exec 3<>/dev/tcp/127.0.0.1/1) 2>/dev/null; then echo "inet=ok"; else echo "inet=blocked"; fi
        echo "memlock=$(ulimit -l)"
        echo "core=$(ulimit -c)"
        echo "umask=$(umask)"
      '';
    in {
      virtualisation.memorySize = 2048;

      users.users.${user} = {
        isNormalUser = true;
        inherit uid;
        linger = true;
      };

      environment.systemPackages = [
        spool
        pkgs.wl-clipboard
        pkgs.wayland-utils
      ];

      systemd.user.targets.sway-session = {
        description = "Headless sway session";
        bindsTo = ["graphical-session.target"];
        wants = ["graphical-session-pre.target"];
        after = ["graphical-session-pre.target"];
      };

      systemd.user.services.sway = {
        description = "Headless sway (test compositor)";
        wantedBy = ["default.target"];
        environment = {
          WLR_BACKENDS = "headless";
          WLR_RENDERER = "pixman";
          WLR_LIBINPUT_NO_DEVICES = "1";
        };
        # sway runs `exec` lines through `sh -c`, found on PATH.
        path = [pkgs.bash];
        serviceConfig = {
          ExecStart = "${pkgs.sway-unwrapped}/bin/sway -c ${swayConfig}";
          Restart = "no";
        };
      };

      # Stand-in for KWallet: a fresh gnome-keyring whose `login` collection
      # is created and unlocked from stdin (same approach as
      # crates/spool-keys/tests/secret-service-harness.sh).
      systemd.user.services.test-keyring = {
        description = "Throwaway unlocked Secret Service";
        wantedBy = ["default.target"];
        serviceConfig.ExecStart = "${pkgs.bash}/bin/bash -c 'printf %s spool-test-password | exec ${pkgs.gnome-keyring}/bin/gnome-keyring-daemon --foreground --unlock --components=secrets'";
      };

      systemd.user.services.spool = {
        unitConfig = unitDef.unit;
        serviceConfig = unitDef.service;
        # NixOS units always get Environment=PATH=<coreutils...> from `path`;
        # replace it with exactly the PATH the home-manager module sets.
        environment.PATH = lib.mkForce unitDef.path;
        wantedBy = ["graphical-session.target"];
        # Test-only: the key flow retries anyway, this just avoids a wait.
        wants = ["test-keyring.service"];
        after = ["test-keyring.service"];
      };

      systemd.user.services.spool-sandbox-probe = {
        unitConfig = unitDef.unit // {Description = "spoold sandbox probe";};
        serviceConfig =
          unitDef.service
          // unitDef.namespaced
          // {
            Type = "oneshot";
            Restart = "no";
            # Separate runtime dir so the probe never touches spoold's.
            RuntimeDirectory = "spool-probe";
            RuntimeDirectoryPreserve = "yes";
            ReadWritePaths = "%S/spool %t/spool %t/spool-probe";
            ExecStart = "${probe}";
          };
        path = [pkgs.systemd pkgs.wayland-utils pkgs.coreutils pkgs.bash pkgs.procps];
      };

      environment.etc."spool-test/session.toml".source = sessionConfig;
    };

    testScript = ''
      import json
      import shlex

      ENV = (
          "export XDG_RUNTIME_DIR=${runtime} WAYLAND_DISPLAY=wayland-1 "
          "DBUS_SESSION_BUS_ADDRESS=unix:path=${runtime}/bus; "
      )

      def wrap(cmd):
          return f"su - ${user} -c {shlex.quote(ENV + cmd)}"

      def as_user(cmd, check=True):
          return machine.succeed(wrap(cmd)) if check else machine.execute(wrap(cmd))

      def status():
          return json.loads(as_user("spoolctl status --json"))

      def wait_key_state(want, timeout=60):
          machine.wait_until_succeeds(
              wrap(f"spoolctl status --json | grep -q '\"key_state\":\"{want}\"'"),
              timeout=timeout,
          )

      def wait_current(text, timeout=30):
          machine.wait_until_succeeds(wrap(f"spoolctl current | grep -qx {text}"), timeout=timeout)

      def wl_copy(text):
          # wl-copy forks a server; detach it from su's pipes or this hangs.
          # Text as an argument: stdin must be /dev/null for that.
          as_user(f"wl-copy -- {text} >/dev/null 2>&1 </dev/null")

      machine.wait_for_unit("user@${toString uid}.service")
      machine.wait_until_succeeds("test -S ${runtime}/wayland-1", timeout=60)
      try:
          machine.wait_until_succeeds("test -S ${runtime}/spool/sock", timeout=60)
      except Exception:
          print(machine.execute("journalctl _UID=${toString uid} --no-pager -n 200")[1])
          raise
      as_user("systemctl --user is-active spool.service")
      wait_key_state("ready")
      print(as_user("spoolctl status"))

      with subtest("hardening properties are applied"):
          props = as_user(
              "systemctl --user show spool -p NoNewPrivileges -p PrivateNetwork -p PrivateUsers "
              "-p ProtectSystem -p ReadWritePaths -p MemoryDenyWriteExecute -p LockPersonality "
              "-p RestrictAddressFamilies -p SystemCallFilter -p UMask -p LimitCORE -p LimitMEMLOCK "
              "-p UnsetEnvironment -p Environment -p Slice -p StateDirectory -p RuntimeDirectory "
              "-p RestrictNamespaces -p RestrictRealtime -p RestrictSUIDSGID -p MainPID"
          )
          print(props)
          got = dict(l.split("=", 1) for l in props.strip().splitlines())
          expect = {
              "NoNewPrivileges": "yes",
              "PrivateNetwork": "no",
              "PrivateUsers": "no",
              "ProtectSystem": "no",
              "RestrictRealtime": "yes",
              "RestrictSUIDSGID": "yes",
              "MemoryDenyWriteExecute": "yes",
              "LockPersonality": "yes",
              "RestrictAddressFamilies": "AF_UNIX",
              "UMask": "0077",
              "LimitCORE": "0",
              "Slice": "session.slice",
          }
          for k, v in expect.items():
              assert got.get(k) == v, f"{k}: expected {v!r}, got {got.get(k)!r}"
          assert "LD_PRELOAD" in got["UnsetEnvironment"], got["UnsetEnvironment"]
          paths = [e for e in got["Environment"].split() if e.startswith("PATH=")]
          assert paths == ["PATH=${unitDef.path}"], got["Environment"]
          assert got["SystemCallFilter"] != "", "SystemCallFilter empty"

          # Runtime evidence from the live process, not just unit properties.
          pid = got["MainPID"]
          status_txt = machine.succeed(f"cat /proc/{pid}/status")
          assert "NoNewPrivs:\t1" in status_txt, status_txt
          assert "Seccomp:\t2" in status_txt, status_txt
          limits = machine.succeed(f"grep -E 'core|locked' /proc/{pid}/limits")
          print(limits)
          # LimitMEMLOCK=256M is accepted by the unit but cannot exceed the
          # user manager's own hard limit (8 MiB default); record what applies.
          assert got["LimitMEMLOCK"] == str(256 * 1024 * 1024), got["LimitMEMLOCK"]
          mgr = machine.succeed("systemctl show user@${toString uid}.service -p MainPID --value").strip()
          ns = {}
          for kind in ("user", "net", "mnt"):
              ns[kind] = (
                  machine.succeed(f"readlink /proc/{pid}/ns/{kind}").strip(),
                  machine.succeed(f"readlink /proc/{mgr}/ns/{kind}").strip(),
              )
          print(f"namespaces (daemon, manager): {ns}")
          # No namespace directives -> spoold shares the manager's user
          # namespace, which its /proc/<peer>/root check depends on.
          assert ns["user"][0] == ns["user"][1], "spoold is in a private user namespace"

      with subtest("namespace directives: D-Bus/Wayland fine, peer check broken"):
          as_user("systemctl --user start spool-sandbox-probe.service")
          out = as_user("cat ${runtime}/spool-probe/out")
          print(out)
          # home=/runtime= are only reported: ProtectSystem=strict in a user
          # unit left both writable here (see systemd-service.nix).
          for want in ("dbus=ok", "secrets=ok", "wayland=ok",
                       "rwpath=rw", "inet=blocked", "umask=0077", "core=0",
                       # Why unitDef.namespaced is not applied to spoold:
                       "peerroot=denied"):
              assert want in out, f"{want} missing from probe output"

      with subtest("copy is recorded"):
          wl_copy("hello-spool")
          wait_current("hello-spool")

      with subtest("secret is not stored"):
          before = status()["item_count"]
          wl_copy("${secret}")
          machine.sleep(3)
          assert as_user("spoolctl current").strip() == "hello-spool"
          assert status()["item_count"] == before
          wl_copy("after-secret")
          wait_current("after-secret")
          machine.fail("grep -rqa ${secret} ${state}/")

      with subtest("history is encrypted at rest"):
          st = status()
          assert st["encrypted"] and st["unlocked"], st
          print(machine.succeed("ls -la ${state}"))
          machine.succeed("test -s ${state}/history.db")
          machine.fail("head -c 15 ${state}/history.db | grep -q 'SQLite format 3'")
          machine.fail("grep -rqa hello-spool ${state}/")

      with subtest("history persists across a restart"):
          as_user("systemctl --user restart spool.service")
          machine.wait_until_succeeds("test -S ${runtime}/spool/sock", timeout=30)
          wait_key_state("ready")
          st = status()
          print(st)
          assert st["item_count"] >= 2, st
          assert as_user("spoolctl current").strip() == "after-secret"

      with subtest("key_provider = session keeps history in memory only"):
          as_user("mkdir -p ~/.config/systemd/user/spool.service.d")
          as_user(
              "printf '[Service]\\nEnvironment=SPOOL_CONFIG=/etc/spool-test/session.toml\\n' "
              "> ~/.config/systemd/user/spool.service.d/session.conf"
          )
          as_user("systemctl --user daemon-reload && systemctl --user restart spool.service")
          machine.wait_until_succeeds("test -S ${runtime}/spool/sock", timeout=30)
          wait_key_state("session-only")
          st = status()
          assert not st["encrypted"], st
          wl_copy("session-only-item")
          wait_current("session-only-item")
          as_user("rm ~/.config/systemd/user/spool.service.d/session.conf")
          as_user("systemctl --user daemon-reload && systemctl --user restart spool.service")
          wait_key_state("ready")
          machine.fail("grep -rqa session-only-item ${state}/")

      with subtest("a user-writable PATH entry makes spoold refuse to start"):
          as_user("mkdir -p ~/bin")
          as_user(
              "printf '[Service]\\nEnvironment=PATH=%s/bin:/run/current-system/sw/bin\\nRestart=no\\n' \"$HOME\" "
              "> ~/.config/systemd/user/spool.service.d/insecure-path.conf"
          )
          as_user("systemctl --user daemon-reload")
          as_user("systemctl --user restart spool.service", check=False)
          machine.wait_until_succeeds(wrap("systemctl --user is-failed spool.service"), timeout=30)
          # Volatile journal: user-unit logs land in the system journal.
          journal = machine.succeed("journalctl _SYSTEMD_USER_UNIT=spool.service --no-pager -n 50")
          assert "refusing to start" in journal, journal
          assert "/home/${user}/bin" in journal, journal
          machine.fail("test -S ${runtime}/spool/sock")

          # Remove the override: it starts again.
          as_user("rm ~/.config/systemd/user/spool.service.d/insecure-path.conf")
          as_user("systemctl --user daemon-reload")
          as_user("systemctl --user reset-failed spool.service")
          as_user("systemctl --user start spool.service")
          machine.wait_until_succeeds("test -S ${runtime}/spool/sock", timeout=30)
          wait_key_state("ready")
    '';
  }
