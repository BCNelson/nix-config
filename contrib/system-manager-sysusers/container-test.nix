# Upstream candidate: testFlake/container-tests/sysusers.nix
#
# Register in testFlake/container-tests/default.nix alongside the other tests.
#
# Test that systemd-sysusers creates declared entities and, crucially, leaves
# pre-existing ones completely alone -- GID, members and all. That create-only
# behaviour is the reason to reach for this instead of userborn on a host whose
# /etc/group is owned by the distribution.
{
  forEachDistro,
  ...
}:

forEachDistro "sysusers" {
  modules = [
    (
      _:
      {
        config = {
          systemd.sysusers.rules = [ "g plugdev -" ];

          systemd.sysusers.settings."10-test" = {
            # Requests GID 999, but the test pre-creates this group with a
            # different GID. sysusers must leave it as it found it.
            preexisting.g.id = "999";

            declareduser.u = {
              gecos = "Declared test user";
              home = "/var/lib/declareduser";
            };
          };
        };
      }
    )
  ];
  extraPathsToRegister = [ ];
  testScriptFunction =
    { toplevel, ... }:
    ''
      start_all()

      machine.wait_for_unit("multi-user.target")

      # A group that already exists, with a GID that does NOT match the one the
      # configuration requests, and with a member that the configuration does
      # not know about.
      machine.succeed("groupadd -g 1234 preexisting")
      machine.succeed("useradd -m zimbatm")
      machine.succeed("usermod -aG preexisting zimbatm")

      before = machine.succeed("getent group preexisting").strip()
      print(f"preexisting before activation: {before}")
      assert before.startswith("preexisting:x:1234:"), before
      assert "zimbatm" in before, before

      machine.activate()
      machine.wait_for_unit("system-manager.target")
      print(machine.succeed("systemctl status system-manager-sysusers.service"))

      # Declared entities are created.
      machine.succeed("getent group plugdev")
      machine.succeed("getent passwd declareduser")

      gecos = machine.succeed("getent passwd declareduser").strip()
      print(f"declareduser: {gecos}")
      assert "Declared test user" in gecos, gecos

      # The pre-existing group is untouched: same GID, same members.
      after = machine.succeed("getent group preexisting").strip()
      print(f"preexisting after activation: {after}")
      assert after.startswith("preexisting:x:1234:"), \
          f"sysusers must not renumber an existing group, got: {after}"
      assert "zimbatm" in after, \
          f"sysusers must not drop existing group members, got: {after}"

      # Re-activating is a no-op rather than an error.
      machine.activate()
      machine.wait_for_unit("system-manager.target")
      assert machine.succeed("getent group preexisting").strip() == after

      # Deactivation leaves the accounts in place; sysusers has no removal path,
      # which is the documented trade-off against userborn.
      machine.succeed("${toplevel}/bin/deactivate 2>&1 | tee /tmp/output.log")
      machine.succeed("! grep -F 'ERROR' /tmp/output.log")
      machine.succeed("getent group plugdev")

      print("SUCCESS: sysusers created declared entities and preserved existing ones")
    '';
}
