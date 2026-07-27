# gamestream-agent

A small MQTT ↔ systemd bridge that lets Home Assistant control on-demand
Sunshine game-streaming sessions on a headless host, and reports live state back.

See [`docs/gamestream.md`](../../docs/gamestream.md) for the full design.

## Commands

```
gamestream-agent serve
    Run the daemon: connect to MQTT, publish Home Assistant discovery, and
    start/stop the gamestream@<instance> units in response to switch commands
    while reporting live state back.

gamestream-agent notify --event stream-start|stream-stop [--profile <id>] [--socket <path>]
    Report a Sunshine stream start/stop to the running daemon (called from
    Sunshine's global_prep_cmd hook). An empty --profile targets the active
    session.
```

## Configuration (environment)

| Variable | Default | Meaning |
|---|---|---|
| `GAMESTREAM_MQTT_BROKER` | `192.168.3.6:1883` | broker `host:port` |
| `GAMESTREAM_MQTT_USERNAME` | `gamestream` | MQTT username |
| `GAMESTREAM_MQTT_PASSWORD` | – | MQTT password (inline) |
| `GAMESTREAM_MQTT_PASSWORD_FILE` | – | file to read the password from |
| `GAMESTREAM_PROFILES` | – | `id=instance,id=instance` (e.g. `brad=game-brad,hannah=game-hannah`) |
| `GAMESTREAM_UNIT_TEMPLATE` | `gamestream@%s.service` | systemd unit per instance |
| `GAMESTREAM_TOPIC_PREFIX` | `romeo/gamestream` | MQTT topic prefix |
| `GAMESTREAM_DISCOVERY_PREFIX` | `homeassistant` | HA discovery prefix |
| `GAMESTREAM_NODE_ID` | `romeo_gamestream` | HA device/node id |
| `GAMESTREAM_SOCKET` | `/run/gamestream-agent/notify.sock` | notify unix socket |
| `GAMESTREAM_START_TIMEOUT` | `120s` | start deadline before `error` |

## Architecture

The decision logic is a pure, deterministic state machine (`agent/engine.go`)
with no I/O; all external systems sit behind interfaces (`Publisher`,
`UnitController`) so the core is exhaustively unit-tested. The paho MQTT and
go-systemd D-Bus adapters are thin and covered by the NixOS VM test.

## Testing

```
go test -race ./...            # unit tests (hermetic)
nix build .#gamestream-agent   # builds + runs the suite in checkPhase
nix build .#gamestream-agent-test   # VM end-to-end test (needs KVM)
```
