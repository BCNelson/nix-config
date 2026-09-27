# End-to-end VM test for Conatus. Boots the real romeo module
# (nixos/romeo/services/conatus.nix) with agenix and the data-dir options
# stubbed out, then drives it through nginx: migrations and the admin bootstrap
# ran, a password login works, and an avatar round-trips through RustFS.
#
# Run with: nix build .#conatus-test -L   (needs KVM)
{
  lib,
  testers,
  conatus,
  conatus-ops,
}: let
  host = "tasks.nel.family";
  adminPassword = "test-admin-password";

  # Just enough of agenix / agenix-rekey / age-template for the module to
  # evaluate: every secret and template gets a `.path` under /etc/test-secrets,
  # and whatever else the module sets (rekeyFile, generator, vars, content) is
  # accepted and ignored.
  secretOpt = lib.mkOption {
    default = {};
    type = lib.types.attrsOf (lib.types.submodule ({name, ...}: {
      freeformType = lib.types.attrsOf lib.types.anything;
      options.path = lib.mkOption {
        type = lib.types.str;
        default = "/etc/test-secrets/${name}";
      };
    }));
  };
  stubs = {
    options = {
      age.secrets = secretOpt;
      age-template.files = secretOpt;
      data.dirs = lib.mkOption {type = lib.types.attrsOf lib.types.str;};
    };
  };
in
  testers.runNixOSTest {
    name = "conatus";

    node.pkgsReadOnly = false;

    nodes.machine = {pkgs, ...}: {
      imports = [stubs ../../nixos/romeo/services/conatus.nix];

      nixpkgs.overlays = [(_: _: {inherit conatus conatus-ops;})];

      virtualisation.memorySize = 2048;

      data.dirs = {
        level2 = "/srv/level2";
        level3 = "/srv/level3";
      };

      environment.etc = {
        "test-secrets/conatus-env".text = ''
          AUTH_SECRET=0000000000000000000000000000000000000000000000000000000000000000
          S3_SECRET_KEY=1111111111111111111111111111111111111111111111111111111111111111
          CONATUS_ADMIN_PASSWORD=${adminPassword}
        '';
        "test-secrets/conatus-rustfs-credentials".text = ''
          RUSTFS_ACCESS_KEY=conatus
          RUSTFS_SECRET_KEY=1111111111111111111111111111111111111111111111111111111111111111
        '';
      };

      # Both come from romeo's server role; the module only adds to them.
      services.postgresql.enable = true;
      services.nginx.enable = true;

      # No ACME in the sandbox, and a Secure session cookie would never come
      # back over plain HTTP.
      services.nginx.virtualHosts.${host} = {
        enableACME = lib.mkForce false;
        forceSSL = lib.mkForce false;
      };
      systemd.services.conatus.environment = {
        AUTH_URL = lib.mkForce "http://${host}";
        PUBLIC_BASE_URL = lib.mkForce "http://${host}";
      };

      environment.systemPackages = [pkgs.curl pkgs.jq pkgs.imagemagick];
    };

    testScript = ''
      curl = "curl -sf --resolve ${host}:80:127.0.0.1"
      base = "http://${host}"

      machine.wait_for_unit("rustfs.service")
      machine.wait_for_unit("conatus.service")
      machine.wait_for_open_port(4399)

      with subtest("health through nginx"):
          machine.wait_for_unit("nginx.service")
          machine.wait_until_succeeds(f"{curl} {base}/api/health | grep -q '\"db\":\"up\"'", timeout=60)

      with subtest("migrations and admin bootstrap ran"):
          machine.succeed("journalctl -u conatus | grep -q 'Database migrations completed.'")
          machine.succeed("journalctl -u conatus | grep -q 'Bootstrap administrator created for bcnelson.'")

      with subtest("password login"):
          machine.succeed(
              f"tok=$({curl} -c /tmp/jar {base}/api/auth/csrf | jq -r .csrfToken) && "
              f"{curl} -b /tmp/jar -c /tmp/jar -o /dev/null -X POST "
              "--data-urlencode csrfToken=$tok --data-urlencode username=bcnelson "
              "--data-urlencode password=${adminPassword} "
              f"{base}/api/auth/callback/credentials"
          )
          machine.succeed("grep -q authjs.session-token /tmp/jar")

      with subtest("avatar round-trips through RustFS"):
          machine.succeed("magick -size 8x8 xc:red /tmp/a.png")
          machine.succeed(f"{curl} -b /tmp/jar -F 'file=@/tmp/a.png;type=image/png' {base}/api/account/avatar")
          machine.succeed(f"{curl} -b /tmp/jar -o /tmp/b.png {base}/api/account/avatar")
          machine.succeed("cmp /tmp/a.png /tmp/b.png")
          machine.succeed("test -d /srv/level3/conatus/rustfs/attachments")

      with subtest("restart is idempotent"):
          machine.succeed("systemctl restart conatus.service")
          machine.wait_for_open_port(4399)
          machine.wait_until_succeeds(f"{curl} {base}/api/health")
          machine.succeed("journalctl -u conatus | grep -q 'Admin bootstrap skipped: the server already has an account.'")
    '';
  }
