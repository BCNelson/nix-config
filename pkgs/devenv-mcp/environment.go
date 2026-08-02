package main

import (
	"context"
	"fmt"
	"strconv"
	"strings"
	"sync"
	"time"
)

// EnvironmentStatus tracks the lifecycle state of a dev environment.
type EnvironmentStatus string

const (
	StatusCreating EnvironmentStatus = "creating"
	StatusReady    EnvironmentStatus = "ready"
	StatusError    EnvironmentStatus = "error"
)

// Environment is a managed development container with an optional Nix devenv.
// Environment fields above the mutex are set once at creation and are safe to
// read without locking. Everything below it is written by the asynchronous
// bootstrap goroutine while tool handlers may be reading, so it is guarded.
type Environment struct {
	ID          string
	Name        string
	ContainerID ContainerID
	RepoURL     string
	WorkDir     string
	CreatedAt   time.Time

	mu       sync.Mutex
	status   EnvironmentStatus
	errMsg   string
	template string
	buildCh  chan struct{} // closed when bootstrap finishes
}

// Template returns the template applied to this environment, which is only
// final once bootstrap has finished (auto-detection resolves it).
func (e *Environment) Template() string {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.template
}

func (e *Environment) setTemplate(name string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.template = name
}

// Status returns the current environment status.
func (e *Environment) Status() EnvironmentStatus {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.status
}

// Error returns the error message if status is StatusError.
func (e *Environment) Error() string {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.errMsg
}

func (e *Environment) setReady() {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.status = StatusReady
}

func (e *Environment) setError(msg string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.status = StatusError
	e.errMsg = msg
}

// nixVolume is the named Podman volume shared across all environments. It
// holds the entire /nix tree (store, database, locks). Concurrent access is
// safe on local filesystems because Nix's SQLite database uses WAL mode and
// fcntl locking, and Nix's own build locks (/nix/var/nix/locks/) prevent
// duplicate builds. Podman named volumes are backed by local storage, so the
// locking primitives work correctly.
const nixVolume = "devenv-mcp-nix"

// EnvironmentRegistry manages all active development environments.
type EnvironmentRegistry struct {
	mu       sync.Mutex
	counter  int
	envs     map[string]*Environment
	provider ContainerProvider
	image    string
	workDir  string
	maxEnvs  int
}

// NewEnvironmentRegistry creates a registry backed by the given provider.
func NewEnvironmentRegistry(provider ContainerProvider, image, workDir string, maxEnvs int) *EnvironmentRegistry {
	return &EnvironmentRegistry{
		envs:     map[string]*Environment{},
		provider: provider,
		image:    image,
		workDir:  workDir,
		maxEnvs:  maxEnvs,
	}
}

// ensureNixVolume creates the shared /nix volume if it doesn't already exist.
// On first use Podman seeds it from the base image's /nix contents.
func (r *EnvironmentRegistry) ensureNixVolume(ctx context.Context) error {
	if err := r.provider.EnsureVolume(ctx, nixVolume); err != nil {
		return fmt.Errorf("ensure volume %s: %w", nixVolume, err)
	}
	return nil
}

// Create sets up a new dev environment. It returns immediately with status
// "creating"; the Nix bootstrap runs asynchronously. Callers can check
// env.Status() or wait on env.buildCh.
func (r *EnvironmentRegistry) Create(ctx context.Context, name, gitURL, templateName string) (*Environment, error) {
	r.mu.Lock()
	if len(r.envs) >= r.maxEnvs {
		r.mu.Unlock()
		return nil, fmt.Errorf("maximum %d environments reached", r.maxEnvs)
	}
	r.counter++
	id := fmt.Sprintf("env-%d", r.counter)
	r.mu.Unlock()

	if name == "" && gitURL != "" {
		name = repoName(gitURL)
	}
	if name == "" && templateName != "" {
		name = templateName + "-" + id
	}
	if name == "" {
		// No name given, and neither the URL nor the template yielded one.
		name = id
	}

	containerName := "devenv-mcp-" + id
	labels := map[string]string{
		"devenv-mcp":        "true",
		"devenv-mcp-env-id": id,
	}

	// Ensure shared /nix volume exists before creating the container.
	if err := r.ensureNixVolume(ctx); err != nil {
		return nil, err
	}

	cid, err := r.provider.Create(ctx, CreateOpts{
		Image:  r.image,
		Name:   containerName,
		Labels: labels,
		Volumes: []VolumeMount{
			{Source: nixVolume, Target: "/nix"},
		},
	})
	if err != nil {
		return nil, fmt.Errorf("create container: %w", err)
	}

	if err := r.provider.Start(ctx, cid); err != nil {
		_ = r.provider.Remove(ctx, cid)
		return nil, fmt.Errorf("start container: %w", err)
	}

	env := &Environment{
		ID:          id,
		Name:        name,
		ContainerID: cid,
		template:    templateName,
		RepoURL:     gitURL,
		WorkDir:     r.workDir,
		CreatedAt:   time.Now(),
		status:      StatusCreating,
		buildCh:     make(chan struct{}),
	}

	r.mu.Lock()
	r.envs[id] = env
	r.mu.Unlock()

	// Run bootstrap asynchronously so the tool returns immediately.
	go r.bootstrap(env, gitURL, templateName)

	return env, nil
}

// bootstrap clones the repo and/or injects devenv templates, then marks the
// environment ready. The shared /nix volume means store paths fetched by any
// environment are immediately available to all others.
func (r *EnvironmentRegistry) bootstrap(env *Environment, gitURL, templateName string) {
	defer close(env.buildCh)
	ctx := context.Background()

	// Step 1: Clone repo if provided.
	if gitURL != "" {
		code, output, err := r.provider.Exec(ctx, env.ContainerID,
			[]string{"git", "clone", gitURL, env.WorkDir},
			ExecOpts{})
		if err != nil || code != 0 {
			msg := output
			if err != nil {
				msg = err.Error()
			}
			env.setError(fmt.Sprintf("git clone failed: %s", msg))
			return
		}
	} else {
		// Create workspace directory.
		_, _, _ = r.provider.Exec(ctx, env.ContainerID,
			[]string{"mkdir", "-p", env.WorkDir}, ExecOpts{})
	}

	// Step 2: Determine template — auto-detect if "auto" or empty (when a
	// repo was cloned).
	tmplName := templateName
	if gitURL != "" && (tmplName == "" || tmplName == "auto") {
		tmplName = r.detectTemplate(ctx, env)
	}

	// Check if this is a well-known repo (already has devenv config).
	if _, isWellKnown := wellKnownRepos[templateName]; isWellKnown {
		tmplName = "" // no template injection needed
	}

	// Step 3: Inject template files if we have a template. Record the resolved
	// name either way, so an environment never reports a template ("auto", or
	// a well-known repo's own config) that was not actually injected.
	if tmplName != "" {
		if err := r.injectTemplate(ctx, env, tmplName); err != nil {
			env.setError(fmt.Sprintf("inject template: %s", err))
			return
		}
	}
	env.setTemplate(tmplName)

	env.setReady()
}

// detectTemplate examines the workspace for indicator files and returns the
// matching template name, or "" if none match.
func (r *EnvironmentRegistry) detectTemplate(ctx context.Context, env *Environment) string {
	for _, t := range builtinTemplates {
		for _, indicator := range t.Indicators {
			code, _, _ := r.provider.Exec(ctx, env.ContainerID,
				[]string{"test", "-f", env.WorkDir + "/" + indicator}, ExecOpts{})
			if code == 0 {
				return t.Name
			}
		}
	}
	return ""
}

// injectTemplate copies the shared flake.nix, .envrc, and the template-specific
// devenv.nix into the workspace.
func (r *EnvironmentRegistry) injectTemplate(ctx context.Context, env *Environment, tmplName string) error {
	sources := []struct{ src, dst string }{
		{"templates/flake.nix", env.WorkDir + "/flake.nix"},
		{"templates/envrc", env.WorkDir + "/.envrc"},
		{"templates/" + tmplName + "/devenv.nix", env.WorkDir + "/devenv.nix"},
	}

	// Read everything first: an unknown template must not leave a workspace
	// with a flake.nix but no devenv.nix.
	files := make([][2]string, 0, len(sources))
	for _, s := range sources {
		content, err := templateFS.ReadFile(s.src)
		if err != nil {
			return fmt.Errorf("read embedded %s: %w", s.src, err)
		}
		files = append(files, [2]string{s.dst, string(content)})
	}

	for _, f := range files {
		if err := r.provider.WriteFile(ctx, env.ContainerID, f[0], []byte(f[1])); err != nil {
			return fmt.Errorf("write %s: %w", f[0], err)
		}
	}

	return nil
}

// List returns all tracked environments.
func (r *EnvironmentRegistry) List() []*Environment {
	r.mu.Lock()
	defer r.mu.Unlock()
	envs := make([]*Environment, 0, len(r.envs))
	for _, e := range r.envs {
		envs = append(envs, e)
	}
	return envs
}

// Get returns an environment by ID.
func (r *EnvironmentRegistry) Get(id string) (*Environment, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	e, ok := r.envs[id]
	return e, ok
}

// Destroy stops and removes an environment's container and unregisters it.
func (r *EnvironmentRegistry) Destroy(ctx context.Context, id string) error {
	r.mu.Lock()
	env, ok := r.envs[id]
	if !ok {
		r.mu.Unlock()
		return fmt.Errorf("unknown environment %q", id)
	}
	delete(r.envs, id)
	r.mu.Unlock()

	if err := r.provider.Remove(ctx, env.ContainerID); err != nil {
		return fmt.Errorf("remove container: %w", err)
	}
	return nil
}

// Reconcile recovers environments from containers that survived a server
// restart by scanning for containers with the devenv-mcp label.
func (r *EnvironmentRegistry) Reconcile(ctx context.Context) error {
	containers, err := r.provider.List(ctx, map[string]string{"devenv-mcp": "true"})
	if err != nil {
		return err
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, c := range containers {
		envID := c.Labels["devenv-mcp-env-id"]
		if envID == "" || r.envs[envID] != nil {
			continue
		}
		running, _ := r.provider.IsRunning(ctx, c.ID)
		status := StatusReady
		if !running {
			status = StatusError
		}
		r.envs[envID] = &Environment{
			ID:          envID,
			Name:        c.Name,
			ContainerID: c.ID,
			WorkDir:     r.workDir,
			CreatedAt:   time.Now(), // approximate
			status:      status,
			buildCh:     make(chan struct{}),
		}
		// Mark as already built.
		close(r.envs[envID].buildCh)

		// Advance the counter past every recovered ID so the next Create does
		// not reissue an ID (and container name) that is already in use.
		if n, ok := envIDNumber(envID); ok && n > r.counter {
			r.counter = n
		}
	}
	return nil
}

// envIDNumber extracts N from an "env-N" identifier.
func envIDNumber(id string) (int, bool) {
	rest, ok := strings.CutPrefix(id, "env-")
	if !ok || rest == "" {
		return 0, false
	}
	n, err := strconv.Atoi(rest)
	if err != nil || n < 0 {
		return 0, false
	}
	return n, true
}

// repoName extracts a short name from a git URL. It returns "" for URLs with
// no final path segment; callers fall back to the environment ID.
func repoName(url string) string {
	// Handle both https://...repo.git and git@...repo.git
	url = strings.TrimSuffix(url, ".git")
	parts := strings.Split(url, "/") // always returns at least one element
	return parts[len(parts)-1]
}
