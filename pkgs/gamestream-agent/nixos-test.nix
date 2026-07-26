# End-to-end VM test for gamestream-agent: boots a machine with an MQTT broker,
# a dummy templated session unit, and the agent running as an unprivileged user
# with a scoped polkit rule, then drives the full control plane over MQTT.
#
# This exercises the parts the Go unit tests can't: the real paho MQTT client,
# the real systemd D-Bus adapter, the polkit scoping, and the notify socket.
#
# Run with: nix build .#gamestream-agent-test -L   (needs KVM)
{
  testers,
  gamestream-agent,
}:
testers.runNixOSTest {
  name = "gamestream-agent";

  nodes.machine =
    { pkgs, ... }:
    {
      # --- MQTT broker (stands in for Home Assistant's Mosquitto) ---
      services.mosquitto = {
        enable = true;
        listeners = [
          {
            address = "127.0.0.1";
            port = 1883;
            users.gamestream = {
              password = "test";
              acl = [ "readwrite #" ];
            };
          }
        ];
      };

      # --- Dummy session unit the agent will start/stop ---
      # A trivial long-running service so ActiveState transitions the same way a
      # real gamestream@ session would.
      systemd.services."gamestream@" = {
        description = "Dummy gamestream session for %i";
        serviceConfig = {
          Type = "simple";
          ExecStart = "${pkgs.coreutils}/bin/sleep infinity";
        };
      };

      # --- Unprivileged agent user + scoped polkit ---
      users.groups.gamestream-agent = { };
      users.users.gamestream-agent = {
        isSystemUser = true;
        group = "gamestream-agent";
      };
      # Allow only this user to manage only the gamestream@ units.
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

      systemd.services.gamestream-agent = {
        description = "gamestream MQTT<->systemd agent";
        wantedBy = [ "multi-user.target" ];
        after = [ "mosquitto.service" ];
        serviceConfig = {
          User = "gamestream-agent";
          Group = "gamestream-agent";
          ExecStart = "${gamestream-agent}/bin/gamestream-agent serve";
          RuntimeDirectory = "gamestream-agent";
          Restart = "on-failure";
        };
        environment = {
          GAMESTREAM_MQTT_BROKER = "127.0.0.1:1883";
          GAMESTREAM_MQTT_USERNAME = "gamestream";
          GAMESTREAM_MQTT_PASSWORD = "test";
          GAMESTREAM_PROFILES = "brad=dummy";
          GAMESTREAM_SOCKET = "/run/gamestream-agent/notify.sock";
          GAMESTREAM_START_TIMEOUT = "60s";
        };
      };

      environment.systemPackages = [
        pkgs.mosquitto
        gamestream-agent
      ];
    };

  testScript = ''
    machine.wait_for_unit("mosquitto.service")
    machine.wait_for_unit("gamestream-agent.service")

    def sub(topic):
        return machine.succeed(
            f"mosquitto_sub -h 127.0.0.1 -u gamestream -P test -C 1 -W 5 -t {topic}"
        ).strip()

    def pub(topic, msg):
        machine.succeed(
            f"mosquitto_pub -h 127.0.0.1 -u gamestream -P test -t {topic} -m {msg}"
        )

    # Discovery is published retained on connect.
    machine.wait_until_succeeds(
        "mosquitto_sub -h 127.0.0.1 -u gamestream -P test -C 1 -W 5 "
        "-t homeassistant/switch/romeo_gamestream_brad/config >/dev/null"
    )

    # Turn the profile ON: the agent must start the dummy unit (proves control +
    # polkit) and report ready.
    pub("romeo/gamestream/brad/set", "ON")
    machine.wait_until_succeeds("systemctl is-active gamestream@dummy.service")
    machine.wait_until_succeeds(
        "test \"$(mosquitto_sub -h 127.0.0.1 -u gamestream -P test -C 1 -W 5 "
        "-t romeo/gamestream/brad/state)\" = ready"
    )

    # Sunshine reports a client connected -> streaming (empty profile routes to
    # the active session).
    machine.succeed(
        "gamestream-agent notify --event stream-start "
        "--socket /run/gamestream-agent/notify.sock"
    )
    machine.wait_until_succeeds(
        "test \"$(mosquitto_sub -h 127.0.0.1 -u gamestream -P test -C 1 -W 5 "
        "-t romeo/gamestream/brad/state)\" = streaming"
    )

    # Client disconnects -> back to ready.
    machine.succeed(
        "gamestream-agent notify --event stream-stop "
        "--socket /run/gamestream-agent/notify.sock"
    )
    machine.wait_until_succeeds(
        "test \"$(mosquitto_sub -h 127.0.0.1 -u gamestream -P test -C 1 -W 5 "
        "-t romeo/gamestream/brad/state)\" = ready"
    )

    # Turn OFF: unit stops and state returns to off.
    pub("romeo/gamestream/brad/set", "OFF")
    machine.wait_until_succeeds("! systemctl is-active gamestream@dummy.service")
    machine.wait_until_succeeds(
        "test \"$(mosquitto_sub -h 127.0.0.1 -u gamestream -P test -C 1 -W 5 "
        "-t romeo/gamestream/brad/state)\" = off"
    )
  '';
}
