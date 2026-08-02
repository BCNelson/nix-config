package main

import (
	"context"
	"errors"
	"strings"
	"testing"
)

// execScript builds an exec hook from a per-command decision function, keyed on
// the first argument (git, mkdir, test, ...).
func execScript(fn func(cmd []string) (int, string, error)) func(cmd []string) (int, string, error) {
	return fn
}

// detectOnly makes `test -f` succeed for exactly one indicator file and fail
// for everything else, so auto-detection resolves deterministically.
func detectOnly(indicator string) func(cmd []string) (int, string, error) {
	return execScript(func(cmd []string) (int, string, error) {
		if len(cmd) == 3 && cmd[0] == "test" && cmd[1] == "-f" {
			if strings.HasSuffix(cmd[2], "/"+indicator) {
				return 0, "", nil
			}
			return 1, "", nil
		}
		return 0, "", nil
	})
}

func TestEnvironmentStatusAccessors(t *testing.T) {
	env := &Environment{status: StatusCreating}

	if got := env.Status(); got != StatusCreating {
		t.Errorf("Status() = %q, want %q", got, StatusCreating)
	}
	if got := env.Error(); got != "" {
		t.Errorf("Error() = %q, want empty", got)
	}

	env.setError("it broke")
	if got := env.Status(); got != StatusError {
		t.Errorf("Status() = %q, want %q", got, StatusError)
	}
	if got := env.Error(); got != "it broke" {
		t.Errorf("Error() = %q, want %q", got, "it broke")
	}

	env.setReady()
	if got := env.Status(); got != StatusReady {
		t.Errorf("Status() = %q, want %q", got, StatusReady)
	}

	env.setTemplate("go")
	if got := env.Template(); got != "go" {
		t.Errorf("Template() = %q, want %q", got, "go")
	}
}

func TestRegistryCreate(t *testing.T) {
	r, f := newTestRegistry(t)

	env := mustCreate(t, r, "", "", "go")

	if env.ID != "env-1" {
		t.Errorf("ID = %q, want env-1", env.ID)
	}
	if env.Status() != StatusReady {
		t.Fatalf("status = %q (%s), want ready", env.Status(), env.Error())
	}
	if env.WorkDir != "/workspace" {
		t.Errorf("WorkDir = %q, want /workspace", env.WorkDir)
	}
	if env.CreatedAt.IsZero() {
		t.Error("CreatedAt was not set")
	}

	if len(f.volumes) != 1 || f.volumes[0] != nixVolume {
		t.Errorf("volumes = %v, want the shared %q volume", f.volumes, nixVolume)
	}
	if len(f.creates) != 1 {
		t.Fatalf("got %d create calls, want 1", len(f.creates))
	}
	opts := f.creates[0]
	if opts.Name != "devenv-mcp-env-1" {
		t.Errorf("container name = %q, want devenv-mcp-env-1", opts.Name)
	}
	if opts.Image != "test-image" {
		t.Errorf("image = %q, want test-image", opts.Image)
	}
	if opts.Labels["devenv-mcp"] != "true" || opts.Labels["devenv-mcp-env-id"] != "env-1" {
		t.Errorf("labels = %v", opts.Labels)
	}
	if len(opts.Volumes) != 1 || opts.Volumes[0].Source != nixVolume || opts.Volumes[0].Target != "/nix" {
		t.Errorf("volumes = %+v, want the shared /nix mount", opts.Volumes)
	}
	if len(f.starts) != 1 || f.starts[0] != env.ContainerID {
		t.Errorf("starts = %v, want the created container", f.starts)
	}
}

func TestRegistryCreateAssignsSequentialIDs(t *testing.T) {
	r, _ := newTestRegistry(t)

	for _, want := range []string{"env-1", "env-2", "env-3"} {
		env := mustCreate(t, r, "", "", "go")
		if env.ID != want {
			t.Errorf("ID = %q, want %q", env.ID, want)
		}
	}
}

func TestRegistryCreateGeneratesNames(t *testing.T) {
	tests := []struct {
		name     string
		given    string
		gitURL   string
		template string
		want     string
	}{
		{name: "explicit name wins", given: "chosen", gitURL: "https://example.com/r.git", template: "go", want: "chosen"},
		{name: "derived from the repo URL", gitURL: "https://github.com/owner/my-repo.git", want: "my-repo"},
		{name: "derived from the template", template: "rust", want: "rust-env-1"},
		{name: "falls back to the ID", want: "env-1"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			r, f := newTestRegistry(t)
			f.setExec(detectOnly("go.mod"))

			env := mustCreate(t, r, tt.given, tt.gitURL, tt.template)
			if env.Name != tt.want {
				t.Errorf("Name = %q, want %q", env.Name, tt.want)
			}
		})
	}
}

func TestRegistryCreateEnforcesMaxEnvs(t *testing.T) {
	f := newFakeProvider()
	r := NewEnvironmentRegistry(f, "img", "/workspace", 2)

	mustCreate(t, r, "", "", "go")
	mustCreate(t, r, "", "", "go")

	_, err := r.Create(context.Background(), "", "", "go")
	if err == nil {
		t.Fatal("Create: expected the limit to be enforced")
	}
	if !strings.Contains(err.Error(), "maximum 2 environments") {
		t.Errorf("error = %q", err)
	}
	if len(f.creates) != 2 {
		t.Errorf("got %d containers, want 2: the limit must be checked before creating", len(f.creates))
	}
}

func TestRegistryCreateFreesSlotsOnDestroy(t *testing.T) {
	f := newFakeProvider()
	r := NewEnvironmentRegistry(f, "img", "/workspace", 1)

	env := mustCreate(t, r, "", "", "go")
	if err := r.Destroy(context.Background(), env.ID); err != nil {
		t.Fatalf("Destroy: %v", err)
	}
	if _, err := r.Create(context.Background(), "", "", "go"); err != nil {
		t.Errorf("Create after Destroy: %v", err)
	}
}

func TestRegistryCreateErrors(t *testing.T) {
	tests := []struct {
		name    string
		setup   func(*fakeProvider)
		wantErr string
	}{
		{
			name:    "volume failure",
			setup:   func(f *fakeProvider) { f.ensureVolumeErr = errors.New("disk full") },
			wantErr: "ensure volume devenv-mcp-nix",
		},
		{
			name:    "container creation failure",
			setup:   func(f *fakeProvider) { f.createErr = errors.New("name in use") },
			wantErr: "create container",
		},
		{
			name:    "start failure",
			setup:   func(f *fakeProvider) { f.startErr = errors.New("oci runtime error") },
			wantErr: "start container",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			r, f := newTestRegistry(t)
			tt.setup(f)

			_, err := r.Create(context.Background(), "", "", "go")
			if err == nil {
				t.Fatal("Create: expected an error")
			}
			if !strings.Contains(err.Error(), tt.wantErr) {
				t.Errorf("error = %q, want it to contain %q", err, tt.wantErr)
			}
			if len(r.List()) != 0 {
				t.Error("a failed Create must not leave an environment registered")
			}
		})
	}
}

// TestRegistryCreateStartFailureRemovesContainer guards against leaking a
// created-but-never-started container.
func TestRegistryCreateStartFailureRemovesContainer(t *testing.T) {
	r, f := newTestRegistry(t)
	f.startErr = errors.New("oci runtime error")

	if _, err := r.Create(context.Background(), "", "", "go"); err == nil {
		t.Fatal("Create: expected an error")
	}
	if got := f.removedIDs(); len(got) != 1 {
		t.Errorf("removed = %v, want the orphaned container to be cleaned up", got)
	}
}

func TestBootstrapClonesRepo(t *testing.T) {
	r, f := newTestRegistry(t)
	f.setExec(detectOnly("go.mod"))

	env := mustCreate(t, r, "", "https://github.com/owner/repo.git", "auto")
	if env.Status() != StatusReady {
		t.Fatalf("status = %q (%s), want ready", env.Status(), env.Error())
	}

	calls := f.execCalls()
	if len(calls) == 0 {
		t.Fatal("no exec calls recorded")
	}
	want := []string{"git", "clone", "https://github.com/owner/repo.git", "/workspace"}
	if !argsEqual(calls[0].Cmd, want) {
		t.Errorf("first exec = %v, want %v", calls[0].Cmd, want)
	}
	if calls[0].ID != env.ContainerID {
		t.Errorf("exec targeted %q, want %q", calls[0].ID, env.ContainerID)
	}
}

func TestBootstrapCreatesWorkspaceWithoutRepo(t *testing.T) {
	r, f := newTestRegistry(t)

	mustCreate(t, r, "", "", "go")

	calls := f.execCalls()
	if len(calls) == 0 {
		t.Fatal("no exec calls recorded")
	}
	if !argsEqual(calls[0].Cmd, []string{"mkdir", "-p", "/workspace"}) {
		t.Errorf("first exec = %v, want mkdir -p /workspace", calls[0].Cmd)
	}
}

func TestBootstrapCloneFailure(t *testing.T) {
	tests := []struct {
		name    string
		exec    func(cmd []string) (int, string, error)
		wantErr string
	}{
		{
			name: "non-zero exit reports git output",
			exec: func(cmd []string) (int, string, error) {
				return 128, "fatal: repository not found", nil
			},
			wantErr: "fatal: repository not found",
		},
		{
			name: "provider error is reported",
			exec: func(cmd []string) (int, string, error) {
				return 0, "", errors.New("podman is not running")
			},
			wantErr: "podman is not running",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			r, f := newTestRegistry(t)
			f.setExec(tt.exec)

			env := mustCreate(t, r, "", "https://example.com/gone.git", "")
			if env.Status() != StatusError {
				t.Fatalf("status = %q, want error", env.Status())
			}
			if !strings.Contains(env.Error(), "git clone failed") || !strings.Contains(env.Error(), tt.wantErr) {
				t.Errorf("error = %q, want it to contain %q", env.Error(), tt.wantErr)
			}
			if len(f.fileWrites()) != 0 {
				t.Error("no template should be injected after a failed clone")
			}
		})
	}
}

func TestBootstrapAutoDetectsTemplate(t *testing.T) {
	tests := []struct {
		indicator string
		want      string
	}{
		{"go.mod", "go"},
		{"Cargo.toml", "rust"},
		{"pyproject.toml", "python"},
		{"requirements.txt", "python"},
		{"pnpm-lock.yaml", "node-pnpm"},
		{"yarn.lock", "node-yarn"},
		{"package-lock.json", "node-npm"},
	}

	for _, tt := range tests {
		t.Run(tt.indicator, func(t *testing.T) {
			r, f := newTestRegistry(t)
			f.setExec(detectOnly(tt.indicator))

			env := mustCreate(t, r, "", "https://example.com/r.git", "auto")
			if env.Status() != StatusReady {
				t.Fatalf("status = %q (%s)", env.Status(), env.Error())
			}
			if got := env.Template(); got != tt.want {
				t.Errorf("detected template = %q, want %q", got, tt.want)
			}

			wantDevenv, err := templateFS.ReadFile("templates/" + tt.want + "/devenv.nix")
			if err != nil {
				t.Fatal(err)
			}
			for _, w := range f.fileWrites() {
				if w.Path == "/workspace/devenv.nix" && w.Content != string(wantDevenv) {
					t.Error("injected devenv.nix does not match the detected template")
				}
			}
		})
	}
}

func TestBootstrapAutoDetectFallsBackToNoTemplate(t *testing.T) {
	r, f := newTestRegistry(t)
	f.setExec(detectOnly("nothing-matches"))

	env := mustCreate(t, r, "", "https://example.com/r.git", "auto")

	if env.Status() != StatusReady {
		t.Fatalf("status = %q (%s), want ready", env.Status(), env.Error())
	}
	if got := env.Template(); got != "" {
		t.Errorf("Template() = %q, want empty when nothing was detected", got)
	}
	if len(f.fileWrites()) != 0 {
		t.Errorf("no files should be injected, got %v", f.writtenPaths())
	}
}

// TestBootstrapDetectionIsSkippedWithoutRepo covers the case where a template
// is named explicitly: no probing should happen.
func TestBootstrapExplicitTemplateSkipsDetection(t *testing.T) {
	r, f := newTestRegistry(t)

	env := mustCreate(t, r, "", "", "rust")

	if env.Template() != "rust" {
		t.Errorf("Template() = %q, want rust", env.Template())
	}
	for _, c := range f.execCalls() {
		if len(c.Cmd) > 0 && c.Cmd[0] == "test" {
			t.Errorf("unexpected detection probe %v for an explicit template", c.Cmd)
		}
	}
}

func TestBootstrapWellKnownRepoSkipsInjection(t *testing.T) {
	restore := withWellKnownRepo(t, "demo-repo", "https://example.com/demo.git")
	defer restore()

	r, f := newTestRegistry(t)
	env := mustCreate(t, r, "", "https://example.com/demo.git", "demo-repo")

	if env.Status() != StatusReady {
		t.Fatalf("status = %q (%s)", env.Status(), env.Error())
	}
	if len(f.fileWrites()) != 0 {
		t.Errorf("well-known repos ship their own devenv config, got writes to %v", f.writtenPaths())
	}
	if got := env.Template(); got != "" {
		t.Errorf("Template() = %q, want empty for a well-known repo", got)
	}
}

func TestInjectTemplateWritesAllFiles(t *testing.T) {
	r, f := newTestRegistry(t)

	mustCreate(t, r, "", "", "go")

	writes := f.fileWrites()
	if len(writes) != 3 {
		t.Fatalf("got %d writes, want 3 (flake.nix, .envrc, devenv.nix): %v", len(writes), f.writtenPaths())
	}

	byPath := map[string]string{}
	for _, w := range writes {
		byPath[w.Path] = w.Content
	}
	for path, embedded := range map[string]string{
		"/workspace/flake.nix":  "templates/flake.nix",
		"/workspace/.envrc":     "templates/envrc",
		"/workspace/devenv.nix": "templates/go/devenv.nix",
	} {
		want, err := templateFS.ReadFile(embedded)
		if err != nil {
			t.Fatal(err)
		}
		got, ok := byPath[path]
		if !ok {
			t.Errorf("nothing written to %s", path)
			continue
		}
		if got != string(want) {
			t.Errorf("%s does not match the embedded %s", path, embedded)
		}
	}
}

func TestInjectTemplateUnknownName(t *testing.T) {
	r, _ := newTestRegistry(t)

	env := mustCreate(t, r, "", "", "haskell")

	if env.Status() != StatusError {
		t.Fatalf("status = %q, want error for a template with no embedded files", env.Status())
	}
	if !strings.Contains(env.Error(), "inject template") || !strings.Contains(env.Error(), "haskell") {
		t.Errorf("error = %q", env.Error())
	}
}

func TestInjectTemplateWriteFailure(t *testing.T) {
	r, f := newTestRegistry(t)
	f.writeErr = errors.New("read-only filesystem")

	env := mustCreate(t, r, "", "", "go")

	if env.Status() != StatusError {
		t.Fatalf("status = %q, want error", env.Status())
	}
	if !strings.Contains(env.Error(), "read-only filesystem") {
		t.Errorf("error = %q, want it to carry the write failure", env.Error())
	}
	if !strings.Contains(env.Error(), "flake.nix") {
		t.Errorf("error = %q, want it to name the file that failed", env.Error())
	}
}

func TestRegistryListAndGet(t *testing.T) {
	r, _ := newTestRegistry(t)

	if got := r.List(); len(got) != 0 {
		t.Errorf("List() on an empty registry = %v", got)
	}
	if _, ok := r.Get("env-1"); ok {
		t.Error("Get returned an environment from an empty registry")
	}

	a := mustCreate(t, r, "first", "", "go")
	b := mustCreate(t, r, "second", "", "rust")

	if got := r.List(); len(got) != 2 {
		t.Errorf("List() returned %d environments, want 2", len(got))
	}

	got, ok := r.Get(a.ID)
	if !ok || got.Name != "first" {
		t.Errorf("Get(%q) = %+v, %v", a.ID, got, ok)
	}
	if got, ok := r.Get(b.ID); !ok || got.Name != "second" {
		t.Errorf("Get(%q) = %+v, %v", b.ID, got, ok)
	}
}

func TestRegistryDestroy(t *testing.T) {
	r, f := newTestRegistry(t)
	env := mustCreate(t, r, "", "", "go")

	if err := r.Destroy(context.Background(), env.ID); err != nil {
		t.Fatalf("Destroy: %v", err)
	}
	if _, ok := r.Get(env.ID); ok {
		t.Error("environment is still registered after Destroy")
	}
	if got := f.removedIDs(); len(got) != 1 || got[0] != env.ContainerID {
		t.Errorf("removed = %v, want %v", got, env.ContainerID)
	}
}

func TestRegistryDestroyUnknown(t *testing.T) {
	r, _ := newTestRegistry(t)

	err := r.Destroy(context.Background(), "env-99")
	if err == nil {
		t.Fatal("Destroy: expected an error")
	}
	if !strings.Contains(err.Error(), `unknown environment "env-99"`) {
		t.Errorf("error = %q", err)
	}
}

func TestRegistryDestroyRemoveFailure(t *testing.T) {
	r, f := newTestRegistry(t)
	env := mustCreate(t, r, "", "", "go")
	f.removeErr = errors.New("container is paused")

	err := r.Destroy(context.Background(), env.ID)
	if err == nil {
		t.Fatal("Destroy: expected an error")
	}
	if !strings.Contains(err.Error(), "remove container") {
		t.Errorf("error = %q", err)
	}
	if _, ok := r.Get(env.ID); ok {
		t.Error("environment should be unregistered even when removal fails")
	}
}

func TestReconcile(t *testing.T) {
	r, f := newTestRegistry(t)
	f.containers = []ContainerInfo{
		{ID: "c1", Name: "devenv-mcp-env-1", Labels: map[string]string{"devenv-mcp-env-id": "env-1"}},
		{ID: "c2", Name: "devenv-mcp-env-2", Labels: map[string]string{"devenv-mcp-env-id": "env-2"}},
		{ID: "c3", Name: "stray", Labels: map[string]string{}},
	}
	f.runningSet = map[ContainerID]bool{"c1": true}

	if err := r.Reconcile(context.Background()); err != nil {
		t.Fatalf("Reconcile: %v", err)
	}

	if got := len(r.List()); got != 2 {
		t.Fatalf("recovered %d environments, want 2 (the unlabeled container is ignored)", got)
	}

	one, ok := r.Get("env-1")
	if !ok {
		t.Fatal("env-1 was not recovered")
	}
	if one.Status() != StatusReady {
		t.Errorf("env-1 status = %q, want ready (its container is running)", one.Status())
	}
	if one.ContainerID != "c1" || one.Name != "devenv-mcp-env-1" || one.WorkDir != "/workspace" {
		t.Errorf("env-1 = %+v", one)
	}
	select {
	case <-one.buildCh:
	default:
		t.Error("a recovered environment must not block callers waiting on bootstrap")
	}

	two, _ := r.Get("env-2")
	if two.Status() != StatusError {
		t.Errorf("env-2 status = %q, want error (its container is not running)", two.Status())
	}

	if f.lastFilters["devenv-mcp"] != "true" {
		t.Errorf("label filter = %v, want devenv-mcp=true", f.lastFilters)
	}
}

// TestReconcileAdvancesCounter is the regression test for ID collisions after a
// restart: the next created environment must not reuse a recovered ID.
func TestReconcileAdvancesCounter(t *testing.T) {
	r, f := newTestRegistry(t)
	f.containers = []ContainerInfo{
		{ID: "c3", Name: "devenv-mcp-env-3", Labels: map[string]string{"devenv-mcp-env-id": "env-3"}},
		{ID: "c1", Name: "devenv-mcp-env-1", Labels: map[string]string{"devenv-mcp-env-id": "env-1"}},
	}

	if err := r.Reconcile(context.Background()); err != nil {
		t.Fatalf("Reconcile: %v", err)
	}

	env := mustCreate(t, r, "", "", "go")
	if env.ID != "env-4" {
		t.Errorf("new environment ID = %q, want env-4 (must not collide with recovered IDs)", env.ID)
	}
	if _, ok := r.Get("env-3"); !ok {
		t.Error("creating a new environment clobbered a recovered one")
	}
}

func TestReconcileSkipsAlreadyTrackedEnvironments(t *testing.T) {
	r, f := newTestRegistry(t)
	env := mustCreate(t, r, "original", "", "go")

	f.containers = []ContainerInfo{
		{ID: "other", Name: "devenv-mcp-env-1", Labels: map[string]string{"devenv-mcp-env-id": env.ID}},
	}
	if err := r.Reconcile(context.Background()); err != nil {
		t.Fatalf("Reconcile: %v", err)
	}

	got, _ := r.Get(env.ID)
	if got.Name != "original" {
		t.Errorf("Name = %q, want the live environment to be preserved", got.Name)
	}
	if len(r.List()) != 1 {
		t.Errorf("got %d environments, want 1", len(r.List()))
	}
}

func TestReconcileIgnoresUnparsableIDs(t *testing.T) {
	r, f := newTestRegistry(t)
	f.containers = []ContainerInfo{
		{ID: "c1", Name: "weird", Labels: map[string]string{"devenv-mcp-env-id": "not-an-env-id"}},
	}

	if err := r.Reconcile(context.Background()); err != nil {
		t.Fatalf("Reconcile: %v", err)
	}
	if _, ok := r.Get("not-an-env-id"); !ok {
		t.Error("the container should still be recovered under its label")
	}

	env := mustCreate(t, r, "", "", "go")
	if env.ID != "env-1" {
		t.Errorf("ID = %q, want env-1: an unparsable ID must not disturb the counter", env.ID)
	}
}

func TestReconcileListError(t *testing.T) {
	r, f := newTestRegistry(t)
	f.listErr = errors.New("podman socket missing")

	err := r.Reconcile(context.Background())
	if err == nil {
		t.Fatal("Reconcile: expected an error")
	}
	if !strings.Contains(err.Error(), "podman socket missing") {
		t.Errorf("error = %q", err)
	}
}

func TestReconcileIsRunningErrorMarksEnvironmentFailed(t *testing.T) {
	r, f := newTestRegistry(t)
	f.containers = []ContainerInfo{
		{ID: "c1", Name: "devenv-mcp-env-1", Labels: map[string]string{"devenv-mcp-env-id": "env-1"}},
	}
	f.isRunningErr = errors.New("inspect failed")

	if err := r.Reconcile(context.Background()); err != nil {
		t.Fatalf("Reconcile: %v", err)
	}
	env, ok := r.Get("env-1")
	if !ok {
		t.Fatal("env-1 was not recovered")
	}
	if env.Status() != StatusError {
		t.Errorf("status = %q, want error when the running check fails", env.Status())
	}
}

func TestEnvIDNumber(t *testing.T) {
	tests := []struct {
		in     string
		want   int
		wantOK bool
	}{
		{"env-1", 1, true},
		{"env-42", 42, true},
		{"env-0", 0, true},
		{"env-", 0, false},
		{"env-abc", 0, false},
		{"env--1", 0, false},
		{"other-1", 0, false},
		{"", 0, false},
		{"1", 0, false},
	}

	for _, tt := range tests {
		t.Run(tt.in, func(t *testing.T) {
			got, ok := envIDNumber(tt.in)
			if got != tt.want || ok != tt.wantOK {
				t.Errorf("envIDNumber(%q) = (%d, %v), want (%d, %v)", tt.in, got, ok, tt.want, tt.wantOK)
			}
		})
	}
}
