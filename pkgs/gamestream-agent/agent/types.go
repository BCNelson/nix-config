// Package agent implements the gamestream control agent: a small daemon that
// bridges Home Assistant (over MQTT) and per-profile game-streaming sessions
// (systemd units), plus the pure state machine that decides what to do.
//
// The design keeps all I/O behind interfaces (Publisher, UnitController) so the
// decision logic (Engine) is a pure, deterministic, exhaustively-tested core.
package agent

import "time"

// State is the lifecycle state of a single gaming profile as reported to Home
// Assistant.
type State string

const (
	// StateOff means nothing is running for the profile (zero idle).
	StateOff State = "off"
	// StateStarting means the session unit was asked to start and we are
	// waiting for it to become active.
	StateStarting State = "starting"
	// StateReady means the session (compositor + Sunshine) is up and awaiting a
	// Moonlight client.
	StateReady State = "ready"
	// StateStreaming means a Moonlight client is actively connected.
	StateStreaming State = "streaming"
	// StateStopping means the session unit was asked to stop.
	StateStopping State = "stopping"
	// StateError means the session failed to come up (or timed out).
	StateError State = "error"
)

// SwitchOn reports whether the Home Assistant switch entity should read ON for
// this state. Starting is reported ON so the toggle feels responsive during the
// spin-up window.
func (s State) SwitchOn() bool {
	switch s {
	case StateStarting, StateReady, StateStreaming:
		return true
	default:
		return false
	}
}

// ActiveState mirrors the subset of systemd unit ActiveState values the engine
// reacts to.
type ActiveState string

const (
	UnitInactive     ActiveState = "inactive"
	UnitActivating   ActiveState = "activating"
	UnitActive       ActiveState = "active"
	UnitDeactivating ActiveState = "deactivating"
	UnitFailed       ActiveState = "failed"
)

// ActionKind identifies the side effect an Action represents.
type ActionKind string

const (
	// ActionStartUnit asks the runtime to start the profile's systemd unit.
	ActionStartUnit ActionKind = "start_unit"
	// ActionStopUnit asks the runtime to stop the profile's systemd unit.
	ActionStopUnit ActionKind = "stop_unit"
	// ActionPublish asks the runtime to publish the profile's current state to
	// MQTT (both the switch and sensor topics).
	ActionPublish ActionKind = "publish_state"
	// ActionSetTimer arms the start-deadline timer for the profile.
	ActionSetTimer ActionKind = "set_timer"
	// ActionCancelTimer disarms the start-deadline timer for the profile.
	ActionCancelTimer ActionKind = "cancel_timer"
)

// Action is a side effect the Engine wants performed. The runtime executes
// them; tests assert on them directly.
type Action struct {
	Kind    ActionKind
	Profile string
	// State is set for ActionPublish and carries the state to publish.
	State State
	// Timeout is set for ActionSetTimer and carries the start deadline.
	Timeout time.Duration
}
