package agent

import "time"

// Engine is the pure state machine at the heart of the agent. It holds the
// per-profile lifecycle state and the single-active invariant, and translates
// external events into a list of Actions for the runtime to execute.
//
// Engine performs no I/O and is not safe for concurrent use; the runtime
// serializes access with a mutex.
type Engine struct {
	order        []string
	profiles     map[string]bool
	states       map[string]State
	active       string // profile that currently owns the session slot, "" if none
	startTimeout time.Duration
}

// NewEngine builds an Engine for the given profile IDs. All profiles start in
// StateOff. profileIDs order is preserved for deterministic InitialActions.
func NewEngine(profileIDs []string, startTimeout time.Duration) *Engine {
	e := &Engine{
		order:        append([]string(nil), profileIDs...),
		profiles:     make(map[string]bool, len(profileIDs)),
		states:       make(map[string]State, len(profileIDs)),
		startTimeout: startTimeout,
	}
	for _, p := range profileIDs {
		e.profiles[p] = true
		e.states[p] = StateOff
	}
	return e
}

// State returns the current state of a profile (StateOff for unknown profiles).
func (e *Engine) State(p string) State {
	if s, ok := e.states[p]; ok {
		return s
	}
	return StateOff
}

// Active returns the profile currently owning the session slot, or "".
func (e *Engine) Active() string { return e.active }

// InitialActions returns the publishes to emit on startup and on every MQTT
// reconnect so Home Assistant always reflects the true state.
func (e *Engine) InitialActions() []Action {
	a := make([]Action, 0, len(e.order))
	for _, p := range e.order {
		a = append(a, e.publish(p))
	}
	return a
}

// OnCommand handles an HA switch command for a profile.
func (e *Engine) OnCommand(p string, on bool) []Action {
	if !e.profiles[p] {
		return nil
	}
	cur := e.states[p]
	if on {
		switch cur {
		case StateStarting, StateReady, StateStreaming, StateStopping:
			// Already on (or mid-stop): re-assert current state, do nothing.
			return []Action{e.publish(p)}
		default: // off, error
			if e.active != "" && e.active != p {
				// Single-active: refuse; another profile owns the slot. Re-assert
				// this profile as off so HA's optimistic ON is corrected.
				return []Action{e.publish(p)}
			}
			e.active = p
			e.set(p, StateStarting)
			return []Action{
				{Kind: ActionStartUnit, Profile: p},
				e.publish(p),
				{Kind: ActionSetTimer, Profile: p, Timeout: e.startTimeout},
			}
		}
	}
	// command OFF
	switch cur {
	case StateStarting, StateReady, StateStreaming:
		e.set(p, StateStopping)
		return []Action{
			{Kind: ActionStopUnit, Profile: p},
			e.publish(p),
			{Kind: ActionCancelTimer, Profile: p},
		}
	default: // off, error, stopping
		if e.active == p {
			e.active = ""
		}
		return []Action{e.publish(p)}
	}
}

// OnUnitEvent handles a systemd ActiveState change for a profile's unit. It also
// reflects externally-driven starts/stops (e.g. via SSH `systemctl`) so HA stays
// truthful regardless of how the session was triggered.
func (e *Engine) OnUnitEvent(p string, st ActiveState) []Action {
	if !e.profiles[p] {
		return nil
	}
	cur := e.states[p]
	switch st {
	case UnitActive:
		if cur == StateStarting {
			e.set(p, StateReady)
			return []Action{e.publish(p), {Kind: ActionCancelTimer, Profile: p}}
		}
	case UnitActivating:
		if cur == StateOff || cur == StateError {
			// Externally started; adopt it, unless another profile owns the slot.
			if e.active != "" && e.active != p {
				return nil
			}
			e.active = p
			e.set(p, StateStarting)
			return []Action{e.publish(p), {Kind: ActionSetTimer, Profile: p, Timeout: e.startTimeout}}
		}
	case UnitDeactivating:
		if cur == StateStarting || cur == StateReady || cur == StateStreaming {
			e.set(p, StateStopping)
			return []Action{e.publish(p), {Kind: ActionCancelTimer, Profile: p}}
		}
	case UnitInactive:
		if cur != StateOff {
			e.set(p, StateOff)
			if e.active == p {
				e.active = ""
			}
			return []Action{e.publish(p), {Kind: ActionCancelTimer, Profile: p}}
		}
	case UnitFailed:
		if cur != StateError {
			e.set(p, StateError)
			if e.active == p {
				e.active = ""
			}
			return []Action{e.publish(p), {Kind: ActionCancelTimer, Profile: p}}
		}
	}
	return nil
}

// OnStreamEvent handles a Sunshine stream start/stop notification.
func (e *Engine) OnStreamEvent(p string, streaming bool) []Action {
	if !e.profiles[p] {
		return nil
	}
	cur := e.states[p]
	if streaming {
		if cur == StateReady {
			e.set(p, StateStreaming)
			return []Action{e.publish(p)}
		}
		return nil
	}
	if cur == StateStreaming {
		e.set(p, StateReady)
		return []Action{e.publish(p)}
	}
	return nil
}

// OnTimeout handles the start-deadline firing for a profile. If the session is
// still only "starting", it is considered failed: we abort the unit and report
// error. The session slot stays owned until the unit is confirmed inactive.
func (e *Engine) OnTimeout(p string) []Action {
	if !e.profiles[p] {
		return nil
	}
	if e.states[p] == StateStarting {
		e.set(p, StateError)
		return []Action{
			{Kind: ActionStopUnit, Profile: p},
			e.publish(p),
		}
	}
	return nil
}

func (e *Engine) set(p string, s State) { e.states[p] = s }

func (e *Engine) publish(p string) Action {
	return Action{Kind: ActionPublish, Profile: p, State: e.states[p]}
}
