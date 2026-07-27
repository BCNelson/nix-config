package agent

import (
	"context"
	"io"
	"log/slog"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

type fakePublisher struct {
	mu        sync.Mutex
	published map[string]string
	subs      map[string]func([]byte)
}

func newFakePublisher() *fakePublisher {
	return &fakePublisher{
		published: map[string]string{},
		subs:      map[string]func([]byte){},
	}
}

func (f *fakePublisher) Publish(topic string, _ bool, payload []byte) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.published[topic] = string(payload)
	return nil
}

func (f *fakePublisher) Subscribe(topic string, handler func([]byte)) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.subs[topic] = handler
	return nil
}

func (f *fakePublisher) inject(topic, payload string) {
	f.mu.Lock()
	h := f.subs[topic]
	f.mu.Unlock()
	if h != nil {
		h([]byte(payload))
	}
}

func (f *fakePublisher) get(topic string) string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.published[topic]
}

type fakeUnits struct {
	mu       sync.Mutex
	started  []string
	stopped  []string
	ch       chan<- UnitStatus
	watching chan struct{}
}

func newFakeUnits() *fakeUnits {
	return &fakeUnits{watching: make(chan struct{})}
}

func (f *fakeUnits) StartUnit(name string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.started = append(f.started, name)
	return nil
}

func (f *fakeUnits) StopUnit(name string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.stopped = append(f.stopped, name)
	return nil
}

func (f *fakeUnits) Watch(ctx context.Context, _ []string, ch chan<- UnitStatus) error {
	f.mu.Lock()
	f.ch = ch
	f.mu.Unlock()
	close(f.watching)
	<-ctx.Done()
	return ctx.Err()
}

func (f *fakeUnits) startedUnits() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]string(nil), f.started...)
}

func eventually(t *testing.T, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatal("condition not met within timeout")
}

func runtimeTestConfig(t *testing.T) Config {
	c := defaultConfig()
	c.SocketPath = filepath.Join(t.TempDir(), "notify.sock")
	c.StartTimeout = time.Hour // long enough not to fire during the test
	c.Profiles = []Profile{{ID: "brad", Instance: "game-brad"}}
	return c
}

func TestRuntimeCommandStartsUnitAndPublishes(t *testing.T) {
	cfg := runtimeTestConfig(t)
	pub := newFakePublisher()
	units := newFakeUnits()
	rt := NewRuntime(cfg, nil, units, slog.New(slog.NewTextHandler(io.Discard, nil)))
	rt.SetPublisher(pub)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go func() { _ = rt.Run(ctx) }()

	<-units.watching // Run has started the watcher and (before it) subscribed
	rt.OnConnect()

	p := cfg.Profiles[0]
	// Initial state published as off.
	eventually(t, func() bool { return pub.get(cfg.SensorStateTopic(p)) == "off" })

	// HA turns the switch ON.
	pub.inject(cfg.CommandTopic(p), "ON")
	eventually(t, func() bool {
		s := units.startedUnits()
		return len(s) == 1 && s[0] == "gamestream@game-brad.service"
	})
	eventually(t, func() bool { return pub.get(cfg.SensorStateTopic(p)) == "starting" })
	eventually(t, func() bool { return pub.get(cfg.SwitchStateTopic(p)) == "ON" })

	// Unit reports active -> ready.
	units.ch <- UnitStatus{Unit: cfg.Unit(p), ActiveState: UnitActive}
	eventually(t, func() bool { return pub.get(cfg.SensorStateTopic(p)) == "ready" })

	// Sunshine reports a client connected -> streaming. An empty profile routes
	// to the active session (brad), mirroring the shared Sunshine hook.
	if err := SendNotify(cfg.SocketPath, NotifyMsg{Streaming: true}, 2*time.Second); err != nil {
		t.Fatal(err)
	}
	eventually(t, func() bool { return pub.get(cfg.SensorStateTopic(p)) == "streaming" })
}

func TestRuntimeReconnectRepublishesDiscovery(t *testing.T) {
	cfg := runtimeTestConfig(t)
	pub := newFakePublisher()
	units := newFakeUnits()
	rt := NewRuntime(cfg, nil, units, slog.New(slog.NewTextHandler(io.Discard, nil)))
	rt.SetPublisher(pub)

	rt.OnConnect()
	p := cfg.Profiles[0]
	if pub.get(cfg.SwitchDiscoveryTopic(p)) == "" {
		t.Fatal("switch discovery not published on connect")
	}
	if pub.get(cfg.AvailabilityTopic()) != "online" {
		t.Fatal("availability not published online on connect")
	}
}
