package agent

import (
	"testing"
	"time"
)

const testTimeout = 90 * time.Second

// newTestEngine returns an engine with two profiles and forces a starting state
// where requested to exercise transitions from arbitrary points.
func newTestEngine(t *testing.T) *Engine {
	t.Helper()
	return NewEngine([]string{"brad", "hannah"}, testTimeout)
}

// drive applies a sequence of events to reach a state, discarding actions.
func kinds(actions []Action) []ActionKind {
	out := make([]ActionKind, len(actions))
	for i, a := range actions {
		out[i] = a.Kind
	}
	return out
}

func hasKind(actions []Action, k ActionKind) bool {
	for _, a := range actions {
		if a.Kind == k {
			return true
		}
	}
	return false
}

func publishState(actions []Action) (State, bool) {
	for _, a := range actions {
		if a.Kind == ActionPublish {
			return a.State, true
		}
	}
	return "", false
}

func TestNewEngineStartsOff(t *testing.T) {
	e := newTestEngine(t)
	if got := e.State("brad"); got != StateOff {
		t.Fatalf("initial state = %q, want off", got)
	}
	if e.Active() != "" {
		t.Fatalf("active = %q, want empty", e.Active())
	}
}

func TestInitialActionsPublishesAllProfilesInOrder(t *testing.T) {
	e := newTestEngine(t)
	got := e.InitialActions()
	if len(got) != 2 {
		t.Fatalf("got %d actions, want 2", len(got))
	}
	if got[0].Profile != "brad" || got[1].Profile != "hannah" {
		t.Fatalf("profiles out of order: %+v", got)
	}
	for _, a := range got {
		if a.Kind != ActionPublish || a.State != StateOff {
			t.Fatalf("unexpected action %+v", a)
		}
	}
}

func TestCommandOnFromOffStarts(t *testing.T) {
	e := newTestEngine(t)
	got := e.OnCommand("brad", true)
	if !hasKind(got, ActionStartUnit) {
		t.Fatalf("want start unit, got %v", kinds(got))
	}
	if !hasKind(got, ActionSetTimer) {
		t.Fatalf("want set timer, got %v", kinds(got))
	}
	if s, _ := publishState(got); s != StateStarting {
		t.Fatalf("published %q, want starting", s)
	}
	if e.State("brad") != StateStarting {
		t.Fatalf("state = %q, want starting", e.State("brad"))
	}
	if e.Active() != "brad" {
		t.Fatalf("active = %q, want brad", e.Active())
	}
	// The set-timer action carries the configured timeout.
	for _, a := range got {
		if a.Kind == ActionSetTimer && a.Timeout != testTimeout {
			t.Fatalf("timer timeout = %v, want %v", a.Timeout, testTimeout)
		}
	}
}

func TestSingleActiveRefusesSecondProfile(t *testing.T) {
	e := newTestEngine(t)
	e.OnCommand("brad", true) // brad now active/starting
	got := e.OnCommand("hannah", true)
	if hasKind(got, ActionStartUnit) {
		t.Fatalf("second profile must not start; got %v", kinds(got))
	}
	// It re-asserts hannah as off so HA's optimistic ON is corrected.
	if s, ok := publishState(got); !ok || s != StateOff {
		t.Fatalf("want republish off for hannah, got %q ok=%v", s, ok)
	}
	if e.State("hannah") != StateOff {
		t.Fatalf("hannah = %q, want off", e.State("hannah"))
	}
	if e.Active() != "brad" {
		t.Fatalf("active = %q, want brad", e.Active())
	}
}

func TestCommandOnIdempotent(t *testing.T) {
	e := newTestEngine(t)
	e.OnCommand("brad", true)
	got := e.OnCommand("brad", true)
	if hasKind(got, ActionStartUnit) {
		t.Fatalf("duplicate ON must not re-start; got %v", kinds(got))
	}
	if len(got) != 1 || got[0].Kind != ActionPublish {
		t.Fatalf("want single republish, got %v", kinds(got))
	}
}

func TestStartingToReadyOnUnitActive(t *testing.T) {
	e := newTestEngine(t)
	e.OnCommand("brad", true)
	got := e.OnUnitEvent("brad", UnitActive)
	if s, _ := publishState(got); s != StateReady {
		t.Fatalf("published %q, want ready", s)
	}
	if !hasKind(got, ActionCancelTimer) {
		t.Fatalf("want cancel timer on ready, got %v", kinds(got))
	}
	if e.State("brad") != StateReady {
		t.Fatalf("state = %q, want ready", e.State("brad"))
	}
}

func TestReadyToStreamingAndBack(t *testing.T) {
	e := newTestEngine(t)
	e.OnCommand("brad", true)
	e.OnUnitEvent("brad", UnitActive)

	got := e.OnStreamEvent("brad", true)
	if s, _ := publishState(got); s != StateStreaming {
		t.Fatalf("published %q, want streaming", s)
	}
	// Stream-start again is a no-op.
	if got := e.OnStreamEvent("brad", true); len(got) != 0 {
		t.Fatalf("duplicate stream-start should be no-op, got %v", kinds(got))
	}

	got = e.OnStreamEvent("brad", false)
	if s, _ := publishState(got); s != StateReady {
		t.Fatalf("published %q, want ready", s)
	}
}

func TestStreamEventIgnoredWhenNotReady(t *testing.T) {
	e := newTestEngine(t)
	if got := e.OnStreamEvent("brad", true); len(got) != 0 {
		t.Fatalf("stream-start while off must be ignored, got %v", kinds(got))
	}
}

func TestCommandOffStops(t *testing.T) {
	e := newTestEngine(t)
	e.OnCommand("brad", true)
	e.OnUnitEvent("brad", UnitActive)
	got := e.OnCommand("brad", false)
	if !hasKind(got, ActionStopUnit) {
		t.Fatalf("want stop unit, got %v", kinds(got))
	}
	if !hasKind(got, ActionCancelTimer) {
		t.Fatalf("want cancel timer, got %v", kinds(got))
	}
	if e.State("brad") != StateStopping {
		t.Fatalf("state = %q, want stopping", e.State("brad"))
	}
}

func TestFullLifecycleReleasesSlot(t *testing.T) {
	e := newTestEngine(t)
	e.OnCommand("brad", true)
	e.OnUnitEvent("brad", UnitActive)
	e.OnStreamEvent("brad", true)
	e.OnStreamEvent("brad", false)
	e.OnCommand("brad", false)
	got := e.OnUnitEvent("brad", UnitInactive)
	if s, _ := publishState(got); s != StateOff {
		t.Fatalf("published %q, want off", s)
	}
	if e.Active() != "" {
		t.Fatalf("active = %q, want empty after teardown", e.Active())
	}
	// Now the other profile may start.
	got = e.OnCommand("hannah", true)
	if !hasKind(got, ActionStartUnit) {
		t.Fatalf("hannah should start after slot freed, got %v", kinds(got))
	}
}

func TestTimeoutFromStartingGoesError(t *testing.T) {
	e := newTestEngine(t)
	e.OnCommand("brad", true)
	got := e.OnTimeout("brad")
	if !hasKind(got, ActionStopUnit) {
		t.Fatalf("timeout should abort with stop unit, got %v", kinds(got))
	}
	if s, _ := publishState(got); s != StateError {
		t.Fatalf("published %q, want error", s)
	}
	if e.State("brad") != StateError {
		t.Fatalf("state = %q, want error", e.State("brad"))
	}
	// Slot stays owned until the unit is confirmed inactive.
	if e.Active() != "brad" {
		t.Fatalf("active = %q, want brad (slot held until inactive)", e.Active())
	}
	// Confirming inactive frees it.
	e.OnUnitEvent("brad", UnitInactive)
	if e.Active() != "" {
		t.Fatalf("active = %q, want empty", e.Active())
	}
}

func TestTimeoutIgnoredWhenNotStarting(t *testing.T) {
	e := newTestEngine(t)
	e.OnCommand("brad", true)
	e.OnUnitEvent("brad", UnitActive) // ready now
	if got := e.OnTimeout("brad"); len(got) != 0 {
		t.Fatalf("timeout while ready must be ignored, got %v", kinds(got))
	}
}

func TestUnitFailedGoesError(t *testing.T) {
	e := newTestEngine(t)
	e.OnCommand("brad", true)
	got := e.OnUnitEvent("brad", UnitFailed)
	if s, _ := publishState(got); s != StateError {
		t.Fatalf("published %q, want error", s)
	}
	if e.Active() != "" {
		t.Fatalf("active = %q, want empty after failure", e.Active())
	}
}

func TestExternalStartAdoptedFromActivating(t *testing.T) {
	e := newTestEngine(t)
	// Simulate an SSH-driven `systemctl start` that the agent observes.
	got := e.OnUnitEvent("brad", UnitActivating)
	if s, _ := publishState(got); s != StateStarting {
		t.Fatalf("published %q, want starting", s)
	}
	if !hasKind(got, ActionSetTimer) {
		t.Fatalf("want set timer for adopted start, got %v", kinds(got))
	}
	if e.Active() != "brad" {
		t.Fatalf("active = %q, want brad", e.Active())
	}
}

func TestExternalStartNotAdoptedWhenSlotBusy(t *testing.T) {
	e := newTestEngine(t)
	e.OnCommand("brad", true)
	if got := e.OnUnitEvent("hannah", UnitActivating); len(got) != 0 {
		t.Fatalf("must not adopt hannah while brad owns slot, got %v", kinds(got))
	}
}

func TestExternalStopReflected(t *testing.T) {
	e := newTestEngine(t)
	e.OnCommand("brad", true)
	e.OnUnitEvent("brad", UnitActive)
	got := e.OnUnitEvent("brad", UnitDeactivating)
	if s, _ := publishState(got); s != StateStopping {
		t.Fatalf("published %q, want stopping", s)
	}
}

func TestUnknownProfileIgnored(t *testing.T) {
	e := newTestEngine(t)
	if got := e.OnCommand("nobody", true); got != nil {
		t.Fatalf("unknown profile command should be nil, got %v", kinds(got))
	}
	if got := e.OnUnitEvent("nobody", UnitActive); got != nil {
		t.Fatalf("unknown profile unit event should be nil, got %v", kinds(got))
	}
	if got := e.OnStreamEvent("nobody", true); got != nil {
		t.Fatalf("unknown profile stream event should be nil, got %v", kinds(got))
	}
	if got := e.OnTimeout("nobody"); got != nil {
		t.Fatalf("unknown profile timeout should be nil, got %v", kinds(got))
	}
}

func TestSwitchOnMapping(t *testing.T) {
	on := map[State]bool{
		StateOff: false, StateStarting: true, StateReady: true,
		StateStreaming: true, StateStopping: false, StateError: false,
	}
	for s, want := range on {
		if s.SwitchOn() != want {
			t.Fatalf("%s.SwitchOn() = %v, want %v", s, s.SwitchOn(), want)
		}
	}
}
