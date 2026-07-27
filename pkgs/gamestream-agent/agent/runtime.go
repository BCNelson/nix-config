package agent

import (
	"context"
	"log/slog"
	"net"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// Publisher is the MQTT surface the runtime depends on. Kept minimal so it can
// be faked in tests.
type Publisher interface {
	Publish(topic string, retained bool, payload []byte) error
	Subscribe(topic string, handler func(payload []byte)) error
}

// UnitController is the systemd surface the runtime depends on.
type UnitController interface {
	StartUnit(name string) error
	StopUnit(name string) error
	// Watch delivers ActiveState changes for the named units on ch until ctx is
	// cancelled.
	Watch(ctx context.Context, units []string, ch chan<- UnitStatus) error
}

// UnitStatus is a single unit ActiveState observation.
type UnitStatus struct {
	Unit        string
	ActiveState ActiveState
}

// Runtime wires the pure Engine to the MQTT and systemd adapters and executes
// the Actions the Engine returns. All Engine access is serialized by mu.
type Runtime struct {
	cfg    Config
	engine *Engine
	mqtt   Publisher
	units  UnitController
	log    *slog.Logger

	mu     sync.Mutex
	timers map[string]*time.Timer

	unitToProfile map[string]Profile
	profileByID   map[string]Profile
}

// NewRuntime constructs a Runtime.
func NewRuntime(cfg Config, mqtt Publisher, units UnitController, log *slog.Logger) *Runtime {
	if log == nil {
		log = slog.Default()
	}
	r := &Runtime{
		cfg:           cfg,
		engine:        NewEngine(cfg.ProfileIDs(), cfg.StartTimeout),
		mqtt:          mqtt,
		units:         units,
		log:           log,
		timers:        map[string]*time.Timer{},
		unitToProfile: map[string]Profile{},
		profileByID:   map[string]Profile{},
	}
	for _, p := range cfg.Profiles {
		r.unitToProfile[cfg.Unit(p)] = p
		r.profileByID[p.ID] = p
	}
	return r
}

// SetPublisher wires the MQTT publisher into the runtime. It must be called
// before the publisher connects, because the connect handler publishes through
// it.
func (r *Runtime) SetPublisher(p Publisher) { r.mqtt = p }

// OnConnect (re)publishes discovery, availability and current state. Safe to
// call on every MQTT (re)connect.
func (r *Runtime) OnConnect() {
	msgs, err := r.cfg.DiscoveryMessages()
	if err != nil {
		r.log.Error("building discovery", "err", err)
	}
	for _, m := range msgs {
		if err := r.mqtt.Publish(m.Topic, true, m.Payload); err != nil {
			r.log.Error("publishing discovery", "topic", m.Topic, "err", err)
		}
	}
	if err := r.mqtt.Publish(r.cfg.AvailabilityTopic(), true, []byte("online")); err != nil {
		r.log.Error("publishing availability", "err", err)
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	r.dispatch(r.engine.InitialActions())
}

// Run subscribes to command topics, starts the unit watcher and the notify
// socket, and blocks until ctx is cancelled.
func (r *Runtime) Run(ctx context.Context) error {
	for _, p := range r.cfg.Profiles {
		p := p
		if err := r.mqtt.Subscribe(r.cfg.CommandTopic(p), func(payload []byte) {
			on := string(payload) == "ON"
			r.mu.Lock()
			r.dispatch(r.engine.OnCommand(p.ID, on))
			r.mu.Unlock()
		}); err != nil {
			return err
		}
	}

	units := make([]string, 0, len(r.cfg.Profiles))
	for _, p := range r.cfg.Profiles {
		units = append(units, r.cfg.Unit(p))
	}
	statusCh := make(chan UnitStatus, 16)
	go func() {
		if err := r.units.Watch(ctx, units, statusCh); err != nil && ctx.Err() == nil {
			r.log.Error("unit watch stopped", "err", err)
		}
	}()
	go r.consumeUnitStatus(ctx, statusCh)

	if err := r.serveNotify(ctx); err != nil {
		return err
	}

	<-ctx.Done()
	// Best-effort graceful availability update.
	_ = r.mqtt.Publish(r.cfg.AvailabilityTopic(), true, []byte("offline"))
	return nil
}

func (r *Runtime) consumeUnitStatus(ctx context.Context, ch <-chan UnitStatus) {
	for {
		select {
		case <-ctx.Done():
			return
		case st, ok := <-ch:
			if !ok {
				return
			}
			p, known := r.unitToProfile[st.Unit]
			if !known {
				continue
			}
			r.mu.Lock()
			r.dispatch(r.engine.OnUnitEvent(p.ID, st.ActiveState))
			r.mu.Unlock()
		}
	}
}

func (r *Runtime) serveNotify(ctx context.Context) error {
	if err := os.MkdirAll(filepath.Dir(r.cfg.SocketPath), 0o755); err != nil {
		return err
	}
	_ = os.Remove(r.cfg.SocketPath)
	l, err := net.Listen("unix", r.cfg.SocketPath)
	if err != nil {
		return err
	}
	// The Sunshine hook runs as the (unprivileged) session user, so the socket
	// must be connectable by it. It carries only stream start/stop for the
	// already-active session, so world-writable is acceptable on a local box.
	if err := os.Chmod(r.cfg.SocketPath, 0o666); err != nil {
		return err
	}
	go func() {
		<-ctx.Done()
		_ = l.Close()
	}()
	go func() {
		_ = ServeNotify(l, func(m NotifyMsg) {
			r.mu.Lock()
			profile := m.Profile
			if profile == "" {
				// Route to whichever profile currently owns the session.
				profile = r.engine.Active()
			}
			if profile != "" {
				r.dispatch(r.engine.OnStreamEvent(profile, m.Streaming))
			}
			r.mu.Unlock()
		})
	}()
	return nil
}

// dispatch executes a batch of Actions. Callers must hold r.mu.
func (r *Runtime) dispatch(actions []Action) {
	for _, a := range actions {
		switch a.Kind {
		case ActionStartUnit:
			r.startUnit(a.Profile)
		case ActionStopUnit:
			r.stopUnit(a.Profile)
		case ActionPublish:
			r.publishState(a.Profile, a.State)
		case ActionSetTimer:
			r.setTimer(a.Profile, a.Timeout)
		case ActionCancelTimer:
			r.cancelTimer(a.Profile)
		}
	}
}

func (r *Runtime) startUnit(id string) {
	unit := r.cfg.Unit(r.profileByID[id])
	go func() {
		if err := r.units.StartUnit(unit); err != nil {
			r.log.Error("starting unit", "unit", unit, "err", err)
			r.mu.Lock()
			r.dispatch(r.engine.OnUnitEvent(id, UnitFailed))
			r.mu.Unlock()
		}
	}()
}

func (r *Runtime) stopUnit(id string) {
	unit := r.cfg.Unit(r.profileByID[id])
	go func() {
		if err := r.units.StopUnit(unit); err != nil {
			r.log.Error("stopping unit", "unit", unit, "err", err)
		}
	}()
}

func (r *Runtime) publishState(id string, s State) {
	p, ok := r.profileByID[id]
	if !ok {
		return
	}
	onoff := "OFF"
	if s.SwitchOn() {
		onoff = "ON"
	}
	if err := r.mqtt.Publish(r.cfg.SwitchStateTopic(p), true, []byte(onoff)); err != nil {
		r.log.Error("publishing switch state", "profile", id, "err", err)
	}
	if err := r.mqtt.Publish(r.cfg.SensorStateTopic(p), true, []byte(string(s))); err != nil {
		r.log.Error("publishing sensor state", "profile", id, "err", err)
	}
}

func (r *Runtime) setTimer(id string, d time.Duration) {
	r.cancelTimer(id)
	r.timers[id] = time.AfterFunc(d, func() {
		r.mu.Lock()
		defer r.mu.Unlock()
		delete(r.timers, id)
		r.dispatch(r.engine.OnTimeout(id))
	})
}

func (r *Runtime) cancelTimer(id string) {
	if t, ok := r.timers[id]; ok {
		t.Stop()
		delete(r.timers, id)
	}
}
