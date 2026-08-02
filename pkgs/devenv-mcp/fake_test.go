package main

import (
	"context"
	"fmt"
	"io"
	"sync"
	"testing"
	"time"
)

// execCall records one Exec invocation against the fake provider.
type execCall struct {
	ID   ContainerID
	Cmd  []string
	Opts ExecOpts
}

// fileWrite records one WriteFile invocation.
type fileWrite struct {
	ID      ContainerID
	Path    string
	Content string
}

// fakeProvider is an in-memory ContainerProvider for testing the registry and
// tool handlers without a container runtime. Every method records its call and
// consults an optional programmable hook or error.
type fakeProvider struct {
	mu sync.Mutex

	volumes []string
	creates []CreateOpts
	starts  []ContainerID
	stops   []ContainerID
	removes []ContainerID
	execs   []execCall
	writes  []fileWrite
	reads   []string

	// Programmable responses. Zero values give a provider where everything
	// succeeds, Exec returns 0 with empty output, and ReadFile returns "".
	ensureVolumeErr error
	createErr       error
	startErr        error
	stopErr         error
	removeErr       error
	listErr         error
	readErr         error
	writeErr        error
	isRunningErr    error

	execFn      func(ctx context.Context, cmd []string) (int, string, error)
	streamFn    func(ctx context.Context, cmd []string, out io.Writer) (int, error)
	readFn      func(path string) ([]byte, error)
	containers  []ContainerInfo
	runningSet  map[ContainerID]bool
	nextID      int
	lastFilters map[string]string
}

func newFakeProvider() *fakeProvider {
	return &fakeProvider{runningSet: map[ContainerID]bool{}}
}

func (f *fakeProvider) EnsureVolume(_ context.Context, name string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.ensureVolumeErr != nil {
		return f.ensureVolumeErr
	}
	f.volumes = append(f.volumes, name)
	return nil
}

func (f *fakeProvider) Create(_ context.Context, opts CreateOpts) (ContainerID, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.createErr != nil {
		return "", f.createErr
	}
	f.creates = append(f.creates, opts)
	f.nextID++
	return ContainerID(fmt.Sprintf("ctr-%d", f.nextID)), nil
}

func (f *fakeProvider) Start(_ context.Context, id ContainerID) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.startErr != nil {
		return f.startErr
	}
	f.starts = append(f.starts, id)
	return nil
}

func (f *fakeProvider) Stop(_ context.Context, id ContainerID) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.stopErr != nil {
		return f.stopErr
	}
	f.stops = append(f.stops, id)
	return nil
}

func (f *fakeProvider) Remove(_ context.Context, id ContainerID) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.removeErr != nil {
		return f.removeErr
	}
	f.removes = append(f.removes, id)
	return nil
}

func (f *fakeProvider) List(_ context.Context, labelFilter map[string]string) ([]ContainerInfo, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.lastFilters = labelFilter
	if f.listErr != nil {
		return nil, f.listErr
	}
	return f.containers, nil
}

func (f *fakeProvider) Exec(ctx context.Context, id ContainerID, cmd []string, opts ExecOpts) (int, string, error) {
	f.mu.Lock()
	f.execs = append(f.execs, execCall{ID: id, Cmd: cmd, Opts: opts})
	fn := f.execFn
	f.mu.Unlock()

	if fn != nil {
		return fn(ctx, cmd)
	}
	return 0, "", nil
}

// ExecStream records the call like Exec and honours streamFn when set, so
// tests can hold a background job open until its context is cancelled.
func (f *fakeProvider) ExecStream(ctx context.Context, id ContainerID, cmd []string, opts ExecOpts, out io.Writer) (int, error) {
	f.mu.Lock()
	f.execs = append(f.execs, execCall{ID: id, Cmd: cmd, Opts: opts})
	stream, fn := f.streamFn, f.execFn
	f.mu.Unlock()

	if stream != nil {
		return stream(ctx, cmd, out)
	}
	if fn != nil {
		code, output, err := fn(ctx, cmd)
		if output != "" {
			_, _ = out.Write([]byte(output))
		}
		return code, err
	}
	return 0, nil
}

// setStream installs a streaming exec hook.
func (f *fakeProvider) setStream(fn func(ctx context.Context, cmd []string, out io.Writer) (int, error)) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.streamFn = fn
}

func (f *fakeProvider) WriteFile(_ context.Context, id ContainerID, path string, content []byte) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.writeErr != nil {
		return f.writeErr
	}
	f.writes = append(f.writes, fileWrite{ID: id, Path: path, Content: string(content)})
	return nil
}

func (f *fakeProvider) ReadFile(_ context.Context, _ ContainerID, path string) ([]byte, error) {
	f.mu.Lock()
	f.reads = append(f.reads, path)
	fn, err := f.readFn, f.readErr
	f.mu.Unlock()

	if err != nil {
		return nil, err
	}
	if fn != nil {
		return fn(path)
	}
	return nil, nil
}

func (f *fakeProvider) IsRunning(_ context.Context, id ContainerID) (bool, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.isRunningErr != nil {
		return false, f.isRunningErr
	}
	return f.runningSet[id], nil
}

// ---- accessors (copy under lock so tests never race the bootstrap goroutine)

func (f *fakeProvider) execCalls() []execCall {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]execCall(nil), f.execs...)
}

func (f *fakeProvider) fileWrites() []fileWrite {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]fileWrite(nil), f.writes...)
}

func (f *fakeProvider) writtenPaths() []string {
	var paths []string
	for _, w := range f.fileWrites() {
		paths = append(paths, w.Path)
	}
	return paths
}

func (f *fakeProvider) removedIDs() []ContainerID {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]ContainerID(nil), f.removes...)
}

// setExec installs an exec hook that ignores the context.
func (f *fakeProvider) setExec(fn func(cmd []string) (int, string, error)) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.execFn = func(_ context.Context, cmd []string) (int, string, error) { return fn(cmd) }
}

// ---- test helpers -----------------------------------------------------------

// newTestRegistry builds a registry over a fresh fake provider.
func newTestRegistry(t *testing.T) (*EnvironmentRegistry, *fakeProvider) {
	t.Helper()
	f := newFakeProvider()
	return NewEnvironmentRegistry(f, "test-image", "/workspace", 10), f
}

// newTestServer builds a tool server over a fresh fake provider. Waits poll on
// a short interval and kills skip most of the SIGTERM grace, so tests that
// exercise timeouts stay fast.
func newTestServer(t *testing.T) (*srv, *fakeProvider) {
	t.Helper()
	r, f := newTestRegistry(t)
	jobs := NewJobRegistry(f, defaultMaxJobs)
	jobs.killGrace = 20 * time.Millisecond
	return &srv{registry: r, jobs: jobs, provider: f, pollBase: time.Millisecond}, f
}

// waitBootstrap blocks until the environment's async bootstrap finishes.
func waitBootstrap(t *testing.T, env *Environment) {
	t.Helper()
	select {
	case <-env.buildCh:
	case <-time.After(5 * time.Second):
		t.Fatalf("bootstrap of %s did not finish within 5s", env.ID)
	}
}

// mustCreate creates an environment and waits for bootstrap to complete.
func mustCreate(t *testing.T, r *EnvironmentRegistry, name, gitURL, tmpl string) *Environment {
	t.Helper()
	env, err := r.Create(context.Background(), name, gitURL, tmpl)
	if err != nil {
		t.Fatalf("Create(%q, %q, %q): %v", name, gitURL, tmpl, err)
	}
	waitBootstrap(t, env)
	return env
}
