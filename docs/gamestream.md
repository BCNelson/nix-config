# Game streaming (romeo → xray)

On-demand, zero-idle game streaming with **romeo** as a headless, server-first
host and the **xray** desktops as clients. Steam games are rendered on romeo and
streamed to Moonlight over the LAN (or Tailscale when remote).

## Decisions

| Layer | Decision |
|---|---|
| Host / posture | romeo, fully headless, server-first; on-demand, **zero idle** |
| Isolation | Dedicated unprivileged users (`game-brad`, `game-hannah`) + a capped `gamestream.slice`. A VM was ruled out: VFIO can't share the GPU with Ollama. |
| GPU split | **Render on the B580** (xe); **capture + VA-API encode on the A380** (i915) — reusing the proven Frigate/Jellyfin encode path. |
| Compositor | Headless `sway` (`WLR_BACKENDS=headless`); gamescope optional per-game. B580 offload via `MESA_VK_DEVICE_SELECT`/`DRI_PRIME` in Steam launch options. |
| Profiles | Two, separate Steam libraries/saves; one active at a time. |
| Trigger | Home Assistant switch + state over MQTT; SSH `systemctl` as fallback. |
| Network | LAN-first + Tailscale for remote; ports not exposed to WAN. |
| Audio | Headless PipeWire null-sink → Sunshine `audio_sink`. |
| Client | `moonlight-qt` on `xray-2` and `xray-3`. |

## Control plane

```
Home Assistant ──MQTT (broker 192.168.3.6:1883)── gamestream-agent (always-on, tiny)
                                                        │ systemd D-Bus
                                                        ▼
                                   gamestream@<user>.service  (dormant until started)
                                     ExecStart: enable-linger + start user session
                                     ExecStop : stop session + disable-linger (zero idle)
                                                        │
                                                        ▼
                                   user session: sway (A380) → Sunshine → Steam
                                                                     game renders on B580
```

- **gamestream-agent** (`pkgs/gamestream-agent`, Go) is the only always-on piece
  (an idle MQTT connection). It publishes Home Assistant MQTT Discovery (a switch
  and a state sensor per profile), starts/stops the `gamestream@<user>` unit in
  response to switch commands, and reports live state (`off → starting → ready →
  streaming → stopping → error`). It also reflects sessions started out-of-band
  over SSH, so HA stays truthful either way. Single-active is enforced: turning on
  a second profile while one is up is refused.
- **Streaming vs. ready** comes from Sunshine's `global_prep_cmd`, which calls
  `gamestream-agent notify` on stream start/stop (routed to the active session).

## Lifecycle

- **On:** HA switch ON → agent `StartUnit(gamestream@game-brad.service)` → linger
  enabled + `gamestream.target` started in the user's manager → sway (headless,
  A380) → Sunshine → Steam Big Picture. State goes `starting → ready`.
- **Streaming:** Moonlight connects → Sunshine `global_prep_cmd` do →
  `gamestream-agent notify --event stream-start` → state `streaming`.
- **Off:** HA switch OFF → agent `StopUnit` → session stopped + linger disabled →
  user manager exits. Nothing left running; GPUs fully released.

## GPU wiring (tuning points)

- Compositor + capture + encode on the **A380** via
  `WLR_RENDER_DRM_DEVICE=/dev/dri/by-driver/i915-render` and Sunshine
  `encoder = vaapi`, `adapter_name = <i915 render node>`.
- Game rendering on the **B580** per-game, e.g. Steam launch options:
  `DRI_PRIME=1 %command%` or `MESA_VK_DEVICE_SELECT=<B580 pci id> %command%`.
- Reverse-PRIME copies each frame B580 → A380; fine at 1080p/1440p, watch at
  4K/high-refresh.

## Secrets / setup (manual, needs hardware keys)

The MQTT password is an agenix secret that must match the `gamestream` user on
the broker. To rotate it, or to re-do this on another host:

1. Create `gamestream` on the broker (192.168.3.6) with a password + ACL scoped
   to `romeo/gamestream/#` and `homeassistant/#`.
2. `agenix edit secrets/store/romeo/gamestream_mqtt_password.age`, `git add` it,
   then `just rekey` to produce the per-host copy under `secrets/hosts/romeo-2/`.

The secret sets `owner = "gamestream-agent"`: unlike Frigate, which gets its
broker password through an `EnvironmentFile` (read by systemd as root), the agent
reads the file itself as its own unprivileged user, so agenix's default
`root:root 0400` is not readable.

## Testing

- **Go unit tests** (`pkgs/gamestream-agent/agent/*_test.go`): the pure engine
  (state machine + single-active), config parsing/validation, HA discovery
  payloads, notify codec, and the runtime wiring with fakes. Run in the package's
  `checkPhase` (CI: *check-gamestream-agent*).
- **VM end-to-end test** (`pkgs/gamestream-agent/nixos-test.nix`): boots MQTT +
  the agent (as an unprivileged user with a scoped polkit rule) + a dummy
  `gamestream@` unit, and drives the full control plane over MQTT (CI:
  *check-gamestream-agent-vm*). Validates the real paho/D-Bus adapters and the
  polkit scoping.

## Known v1 simplifications / follow-ups

- **Shared Sunshine config** (one port base): both profiles present identically
  to Moonlight, so switching profiles re-pairs. A per-user port base (two
  independently-paired Moonlight hosts) is a follow-up.
- Headless-session wiring (sway → graphical-session → Sunshine) and the exact
  virtual-output resolution / VA-API device / audio sink name need on-hardware
  tuning; the values in `gamestream.nix` are marked `HARDWARE:`.
