package main

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// textOf extracts the human-readable text block from a tool result.
func textOf(t *testing.T, res *mcp.CallToolResult) string {
	t.Helper()
	if res == nil {
		t.Fatal("tool result is nil")
	}
	if len(res.Content) != 1 {
		t.Fatalf("got %d content blocks, want 1", len(res.Content))
	}
	tc, ok := res.Content[0].(*mcp.TextContent)
	if !ok {
		t.Fatalf("content is %T, want *mcp.TextContent", res.Content[0])
	}
	return tc.Text
}

// seedEnv creates a ready environment through the registry for handler tests.
func seedEnv(t *testing.T, s *srv) *Environment {
	t.Helper()
	return mustCreate(t, s.registry, "seed", "", "go")
}

// ---- env_create -------------------------------------------------------------

func TestEnvCreate(t *testing.T) {
	s, _ := newTestServer(t)

	res, out, err := s.envCreate(context.Background(), nil, envCreateInput{Template: "go", Name: "my-env"})
	if err != nil {
		t.Fatalf("envCreate: %v", err)
	}
	if out.EnvID != "env-1" {
		t.Errorf("EnvID = %q, want env-1", out.EnvID)
	}
	if out.Name != "my-env" {
		t.Errorf("Name = %q, want my-env", out.Name)
	}
	if out.Status != string(StatusCreating) {
		t.Errorf("Status = %q, want %q: create returns before bootstrap finishes", out.Status, StatusCreating)
	}
	if out.Template != "go" {
		t.Errorf("Template = %q, want go", out.Template)
	}

	text := textOf(t, res)
	if !strings.Contains(text, "env-1") || !strings.Contains(text, "my-env") {
		t.Errorf("text = %q, want it to name the environment", text)
	}

	env, ok := s.registry.Get("env-1")
	if !ok {
		t.Fatal("environment was not registered")
	}
	waitBootstrap(t, env)
}

func TestEnvCreateWithGitURL(t *testing.T) {
	s, f := newTestServer(t)
	f.setExec(detectOnly("go.mod"))

	_, out, err := s.envCreate(context.Background(), nil, envCreateInput{GitURL: "https://github.com/owner/thing.git"})
	if err != nil {
		t.Fatalf("envCreate: %v", err)
	}
	if out.Name != "thing" {
		t.Errorf("Name = %q, want the name derived from the repo", out.Name)
	}
	if out.RepoURL != "https://github.com/owner/thing.git" {
		t.Errorf("RepoURL = %q", out.RepoURL)
	}

	env, _ := s.registry.Get(out.EnvID)
	waitBootstrap(t, env)
}

func TestEnvCreateWellKnownRepoShortcut(t *testing.T) {
	restore := withWellKnownRepo(t, "demo-repo", "https://example.com/demo.git")
	defer restore()

	s, _ := newTestServer(t)

	_, out, err := s.envCreate(context.Background(), nil, envCreateInput{Template: "demo-repo"})
	if err != nil {
		t.Fatalf("envCreate: %v", err)
	}
	if out.RepoURL != "https://example.com/demo.git" {
		t.Errorf("RepoURL = %q, want the shortcut to be expanded", out.RepoURL)
	}

	env, _ := s.registry.Get(out.EnvID)
	waitBootstrap(t, env)
}

func TestEnvCreateValidation(t *testing.T) {
	tests := []struct {
		name    string
		in      envCreateInput
		wantErr string
	}{
		{
			name:    "neither template nor git_url",
			in:      envCreateInput{},
			wantErr: "at least one of git_url or template is required",
		},
		{
			name:    "unknown template",
			in:      envCreateInput{Template: "haskell"},
			wantErr: `unknown template "haskell"`,
		},
		{
			name:    "auto without a repo",
			in:      envCreateInput{Template: "auto"},
			wantErr: "requires git_url",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s, f := newTestServer(t)

			_, _, err := s.envCreate(context.Background(), nil, tt.in)
			if err == nil {
				t.Fatal("envCreate: expected an error")
			}
			if !strings.Contains(err.Error(), tt.wantErr) {
				t.Errorf("error = %q, want it to contain %q", err, tt.wantErr)
			}
			if len(f.creates) != 0 {
				t.Error("no container should be created for an invalid request")
			}
		})
	}
}

func TestEnvCreateRegistryError(t *testing.T) {
	s, f := newTestServer(t)
	f.createErr = errors.New("no space left on device")

	_, _, err := s.envCreate(context.Background(), nil, envCreateInput{Template: "go"})
	if err == nil {
		t.Fatal("envCreate: expected an error")
	}
	if !strings.Contains(err.Error(), "no space left on device") {
		t.Errorf("error = %q", err)
	}
}

// ---- env_list ---------------------------------------------------------------

func TestEnvListEmpty(t *testing.T) {
	s, _ := newTestServer(t)

	res, out, err := s.envList(context.Background(), nil, struct{}{})
	if err != nil {
		t.Fatalf("envList: %v", err)
	}
	if len(out.Environments) != 0 {
		t.Errorf("got %d environments, want 0", len(out.Environments))
	}
	if got := textOf(t, res); got != "no environments" {
		t.Errorf("text = %q, want %q", got, "no environments")
	}
}

func TestEnvListSortsByID(t *testing.T) {
	s, _ := newTestServer(t)
	for i := 0; i < 3; i++ {
		mustCreate(t, s.registry, "", "", "go")
	}

	_, out, err := s.envList(context.Background(), nil, struct{}{})
	if err != nil {
		t.Fatalf("envList: %v", err)
	}
	if len(out.Environments) != 3 {
		t.Fatalf("got %d environments, want 3", len(out.Environments))
	}
	for i, want := range []string{"env-1", "env-2", "env-3"} {
		if out.Environments[i].EnvID != want {
			t.Errorf("environment %d = %q, want %q", i, out.Environments[i].EnvID, want)
		}
	}
}

func TestEnvListReportsFields(t *testing.T) {
	s, f := newTestServer(t)
	f.setExec(detectOnly("go.mod"))
	env := mustCreate(t, s.registry, "named", "https://example.com/r.git", "auto")

	res, out, err := s.envList(context.Background(), nil, struct{}{})
	if err != nil {
		t.Fatalf("envList: %v", err)
	}

	got := out.Environments[0]
	if got.Name != "named" || got.Status != string(StatusReady) || got.Template != "go" {
		t.Errorf("environment = %+v", got)
	}
	if got.RepoURL != "https://example.com/r.git" {
		t.Errorf("RepoURL = %q", got.RepoURL)
	}
	if got.WorkDir != "/workspace" {
		t.Errorf("WorkDir = %q", got.WorkDir)
	}
	if _, err := time.Parse(time.RFC3339, got.CreatedAt); err != nil {
		t.Errorf("CreatedAt = %q, want RFC3339: %v", got.CreatedAt, err)
	}
	if !strings.Contains(textOf(t, res), env.ID) {
		t.Error("rendered text does not mention the environment")
	}
}

func TestEnvListReportsErrors(t *testing.T) {
	s, f := newTestServer(t)
	f.writeErr = errors.New("disk full")
	mustCreate(t, s.registry, "broken", "", "go")

	res, out, err := s.envList(context.Background(), nil, struct{}{})
	if err != nil {
		t.Fatalf("envList: %v", err)
	}
	if out.Environments[0].Status != string(StatusError) {
		t.Errorf("Status = %q, want error", out.Environments[0].Status)
	}
	if !strings.Contains(out.Environments[0].Error, "disk full") {
		t.Errorf("Error = %q", out.Environments[0].Error)
	}
	if !strings.Contains(textOf(t, res), "disk full") {
		t.Error("rendered text should surface the failure")
	}
}

// ---- env_destroy ------------------------------------------------------------

func TestEnvDestroy(t *testing.T) {
	s, _ := newTestServer(t)
	env := seedEnv(t, s)

	res, _, err := s.envDestroy(context.Background(), nil, envIDInput{EnvID: env.ID})
	if err != nil {
		t.Fatalf("envDestroy: %v", err)
	}
	if !strings.Contains(textOf(t, res), env.ID) {
		t.Error("confirmation should name the environment")
	}
	if _, ok := s.registry.Get(env.ID); ok {
		t.Error("environment still registered")
	}
}

func TestEnvDestroyUnknown(t *testing.T) {
	s, _ := newTestServer(t)

	_, _, err := s.envDestroy(context.Background(), nil, envIDInput{EnvID: "env-99"})
	if err == nil {
		t.Fatal("envDestroy: expected an error")
	}
	if !strings.Contains(err.Error(), "unknown environment") {
		t.Errorf("error = %q", err)
	}
}

// ---- env_exec ---------------------------------------------------------------

func TestEnvExec(t *testing.T) {
	s, f := newTestServer(t)
	env := seedEnv(t, s)
	f.setExec(func(cmd []string) (int, string, error) { return 0, "all tests pass\n", nil })

	res, out, err := s.envExec(context.Background(), nil, envExecInput{EnvID: env.ID, Command: "go test ./..."})
	if err != nil {
		t.Fatalf("envExec: %v", err)
	}
	if out.Status != "exited" {
		t.Errorf("Status = %q, want exited", out.Status)
	}
	if out.ExitCode == nil || *out.ExitCode != 0 {
		t.Errorf("ExitCode = %v, want 0", out.ExitCode)
	}
	if out.Output != "all tests pass\n" {
		t.Errorf("Output = %q", out.Output)
	}
	if out.Command != "go test ./..." {
		t.Errorf("Command = %q", out.Command)
	}
	if out.ElapsedSec < 0 {
		t.Errorf("ElapsedSec = %v", out.ElapsedSec)
	}
	if !strings.Contains(textOf(t, res), "all tests pass") {
		t.Error("rendered text should include the output")
	}

	// The command must be run through a shell, in the workspace by default.
	calls := f.execCalls()
	last := calls[len(calls)-1]
	if !argsEqual(last.Cmd, []string{"sh", "-c", "go test ./..."}) {
		t.Errorf("exec cmd = %v", last.Cmd)
	}
	if last.Opts.WorkDir != "/workspace" {
		t.Errorf("WorkDir = %q, want the workspace root by default", last.Opts.WorkDir)
	}
}

func TestEnvExecNonZeroExit(t *testing.T) {
	s, f := newTestServer(t)
	env := seedEnv(t, s)
	f.setExec(func(cmd []string) (int, string, error) { return 2, "build failed", nil })

	_, out, err := s.envExec(context.Background(), nil, envExecInput{EnvID: env.ID, Command: "make"})
	if err != nil {
		t.Fatalf("envExec: %v", err)
	}
	if out.Status != "exited" {
		t.Errorf("Status = %q, want exited: a non-zero exit is not a tool error", out.Status)
	}
	if out.ExitCode == nil || *out.ExitCode != 2 {
		t.Errorf("ExitCode = %v, want 2", out.ExitCode)
	}
}

func TestEnvExecProviderError(t *testing.T) {
	s, f := newTestServer(t)
	env := seedEnv(t, s)
	f.setExec(func(cmd []string) (int, string, error) { return -1, "", errors.New("container is gone") })

	_, out, err := s.envExec(context.Background(), nil, envExecInput{EnvID: env.ID, Command: "ls"})
	if err != nil {
		t.Fatalf("envExec: %v", err)
	}
	if out.Status != "error" {
		t.Errorf("Status = %q, want error", out.Status)
	}
	if !strings.Contains(out.Error, "container is gone") {
		t.Errorf("Error = %q", out.Error)
	}
	if out.ExitCode != nil {
		t.Errorf("ExitCode = %v, want nil when the command never ran", out.ExitCode)
	}
}

// TestEnvExecTimeout is the regression test for the timeout branch: the
// provider kills the process on deadline, which surfaces as an ordinary exit
// rather than an error, so the context must be what decides.
func TestEnvExecTimeout(t *testing.T) {
	s, f := newTestServer(t)
	env := seedEnv(t, s)

	f.mu.Lock()
	f.execFn = func(ctx context.Context, cmd []string) (int, string, error) {
		<-ctx.Done()
		return -1, "partial output", nil // as if the child were killed
	}
	f.mu.Unlock()

	_, out, err := s.envExec(context.Background(), nil, envExecInput{
		EnvID: env.ID, Command: "sleep 600", TimeoutSec: 1,
	})
	if err != nil {
		t.Fatalf("envExec: %v", err)
	}
	if out.Status != "timeout" {
		t.Fatalf("Status = %q, want timeout", out.Status)
	}
	if !strings.Contains(out.Error, "timed out after 1s") {
		t.Errorf("Error = %q, want it to state the timeout", out.Error)
	}
	if out.ExitCode != nil {
		t.Errorf("ExitCode = %v, want nil on timeout", out.ExitCode)
	}
	if out.Output != "partial output" {
		t.Errorf("Output = %q, want the partial output to be preserved", out.Output)
	}
}

func TestEnvExecCustomWorkDir(t *testing.T) {
	s, f := newTestServer(t)
	env := seedEnv(t, s)

	_, _, err := s.envExec(context.Background(), nil, envExecInput{
		EnvID: env.ID, Command: "ls", WorkDir: "/workspace/sub/dir",
	})
	if err != nil {
		t.Fatalf("envExec: %v", err)
	}

	calls := f.execCalls()
	if got := calls[len(calls)-1].Opts.WorkDir; got != "/workspace/sub/dir" {
		t.Errorf("WorkDir = %q", got)
	}
}

func TestEnvExecRejectsWorkDirEscape(t *testing.T) {
	for _, dir := range []string{"/etc", "/workspace/../etc", "/workspace-evil"} {
		t.Run(dir, func(t *testing.T) {
			s, _ := newTestServer(t)
			env := seedEnv(t, s)

			_, _, err := s.envExec(context.Background(), nil, envExecInput{
				EnvID: env.ID, Command: "ls", WorkDir: dir,
			})
			if err == nil {
				t.Fatalf("work_dir %q escaped the workspace without an error", dir)
			}
			if !strings.Contains(err.Error(), "work_dir") {
				t.Errorf("error = %q", err)
			}
		})
	}
}

func TestEnvExecUnknownEnvironment(t *testing.T) {
	s, _ := newTestServer(t)

	_, _, err := s.envExec(context.Background(), nil, envExecInput{EnvID: "env-99", Command: "ls"})
	if err == nil {
		t.Fatal("envExec: expected an error")
	}
	if !strings.Contains(err.Error(), `unknown environment "env-99"`) {
		t.Errorf("error = %q", err)
	}
}

// ---- env_read_file / env_write_file / env_list_dir --------------------------

func TestEnvReadFile(t *testing.T) {
	s, f := newTestServer(t)
	env := seedEnv(t, s)
	f.readFn = func(path string) ([]byte, error) { return []byte("module devenv-mcp\n"), nil }

	res, _, err := s.envReadFile(context.Background(), nil, envFileInput{EnvID: env.ID, Path: "go.mod"})
	if err != nil {
		t.Fatalf("envReadFile: %v", err)
	}
	if got := textOf(t, res); got != "module devenv-mcp\n" {
		t.Errorf("text = %q", got)
	}
	if len(f.reads) != 1 || f.reads[0] != "/workspace/go.mod" {
		t.Errorf("reads = %v, want the path resolved against the workspace", f.reads)
	}
}

func TestEnvReadFileErrors(t *testing.T) {
	t.Run("unknown environment", func(t *testing.T) {
		s, _ := newTestServer(t)
		_, _, err := s.envReadFile(context.Background(), nil, envFileInput{EnvID: "env-99", Path: "x"})
		if err == nil || !strings.Contains(err.Error(), "unknown environment") {
			t.Fatalf("error = %v", err)
		}
	})

	t.Run("provider failure", func(t *testing.T) {
		s, f := newTestServer(t)
		env := seedEnv(t, s)
		f.readErr = errors.New("no such file")

		_, _, err := s.envReadFile(context.Background(), nil, envFileInput{EnvID: env.ID, Path: "gone"})
		if err == nil || !strings.Contains(err.Error(), "no such file") {
			t.Fatalf("error = %v", err)
		}
	})
}

func TestEnvWriteFile(t *testing.T) {
	s, f := newTestServer(t)
	env := seedEnv(t, s)

	res, _, err := s.envWriteFile(context.Background(), nil, envWriteFileInput{
		EnvID: env.ID, Path: "cmd/main.go", Content: "package main",
	})
	if err != nil {
		t.Fatalf("envWriteFile: %v", err)
	}
	if got := textOf(t, res); !strings.Contains(got, "12 bytes") {
		t.Errorf("text = %q, want the byte count", got)
	}

	writes := f.fileWrites()
	last := writes[len(writes)-1]
	if last.Path != "/workspace/cmd/main.go" {
		t.Errorf("path = %q, want it resolved against the workspace", last.Path)
	}
	if last.Content != "package main" {
		t.Errorf("content = %q", last.Content)
	}
}

func TestEnvWriteFileErrors(t *testing.T) {
	t.Run("unknown environment", func(t *testing.T) {
		s, _ := newTestServer(t)
		_, _, err := s.envWriteFile(context.Background(), nil, envWriteFileInput{EnvID: "env-99", Path: "x"})
		if err == nil || !strings.Contains(err.Error(), "unknown environment") {
			t.Fatalf("error = %v", err)
		}
	})

	t.Run("provider failure", func(t *testing.T) {
		s, f := newTestServer(t)
		env := seedEnv(t, s)
		f.writeErr = errors.New("read-only filesystem")

		_, _, err := s.envWriteFile(context.Background(), nil, envWriteFileInput{EnvID: env.ID, Path: "x"})
		if err == nil || !strings.Contains(err.Error(), "read-only filesystem") {
			t.Fatalf("error = %v", err)
		}
	})
}

func TestEnvListDir(t *testing.T) {
	s, f := newTestServer(t)
	env := seedEnv(t, s)
	f.setExec(func(cmd []string) (int, string, error) { return 0, "total 0\ndrwxr-xr-x 2 root root\n", nil })

	res, _, err := s.envListDir(context.Background(), nil, envFileInput{EnvID: env.ID, Path: "sub"})
	if err != nil {
		t.Fatalf("envListDir: %v", err)
	}
	if !strings.Contains(textOf(t, res), "drwxr-xr-x") {
		t.Error("listing was not returned")
	}

	calls := f.execCalls()
	if got := calls[len(calls)-1].Cmd; !argsEqual(got, []string{"ls", "-la", "/workspace/sub"}) {
		t.Errorf("cmd = %v", got)
	}
}

func TestEnvListDirErrors(t *testing.T) {
	tests := []struct {
		name    string
		exec    func(cmd []string) (int, string, error)
		wantErr string
	}{
		{
			name:    "non-zero exit surfaces ls output",
			exec:    func(cmd []string) (int, string, error) { return 2, "ls: no such directory", nil },
			wantErr: "ls: no such directory",
		},
		{
			name:    "provider error surfaces the error",
			exec:    func(cmd []string) (int, string, error) { return -1, "", errors.New("container gone") },
			wantErr: "container gone",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s, f := newTestServer(t)
			env := seedEnv(t, s)
			f.setExec(tt.exec)

			_, _, err := s.envListDir(context.Background(), nil, envFileInput{EnvID: env.ID, Path: "sub"})
			if err == nil {
				t.Fatal("envListDir: expected an error")
			}
			if !strings.Contains(err.Error(), "ls failed") || !strings.Contains(err.Error(), tt.wantErr) {
				t.Errorf("error = %q, want it to contain %q", err, tt.wantErr)
			}
		})
	}

	t.Run("unknown environment", func(t *testing.T) {
		s, _ := newTestServer(t)
		_, _, err := s.envListDir(context.Background(), nil, envFileInput{EnvID: "env-99", Path: "x"})
		if err == nil || !strings.Contains(err.Error(), "unknown environment") {
			t.Fatalf("error = %v", err)
		}
	})
}

// TestFileToolsRejectEscape covers the containment check on every file tool.
func TestFileToolsRejectEscape(t *testing.T) {
	escapes := []string{
		"/etc/shadow",
		"../../etc/shadow",
		"/workspace/../etc/shadow",
		"/workspace-evil/x",
		"",
	}

	tools := map[string]func(*srv, string, string) error{
		"env_read_file": func(s *srv, id, p string) error {
			_, _, err := s.envReadFile(context.Background(), nil, envFileInput{EnvID: id, Path: p})
			return err
		},
		"env_write_file": func(s *srv, id, p string) error {
			_, _, err := s.envWriteFile(context.Background(), nil, envWriteFileInput{EnvID: id, Path: p, Content: "x"})
			return err
		},
		"env_list_dir": func(s *srv, id, p string) error {
			_, _, err := s.envListDir(context.Background(), nil, envFileInput{EnvID: id, Path: p})
			return err
		},
	}

	for tool, call := range tools {
		for _, path := range escapes {
			t.Run(tool+" "+path, func(t *testing.T) {
				s, f := newTestServer(t)
				env := seedEnv(t, s)
				before := len(f.fileWrites())

				if err := call(s, env.ID, path); err == nil {
					t.Fatalf("%s accepted %q, which escapes the workspace", tool, path)
				}
				if len(f.fileWrites()) != before {
					t.Errorf("%s wrote to the container despite rejecting the path", tool)
				}
			})
		}
	}
}

// ---- env_list_templates -----------------------------------------------------

func TestEnvListTemplates(t *testing.T) {
	s, _ := newTestServer(t)

	res, _, err := s.envListTemplates(context.Background(), nil, struct{}{})
	if err != nil {
		t.Fatalf("envListTemplates: %v", err)
	}

	text := textOf(t, res)
	for _, tmpl := range builtinTemplates {
		if !strings.Contains(text, tmpl.Name) {
			t.Errorf("listing omits template %q", tmpl.Name)
		}
		for _, ind := range tmpl.Indicators {
			if !strings.Contains(text, ind) {
				t.Errorf("listing omits indicator %q", ind)
			}
		}
	}
	if strings.Contains(text, "well-known repositories") {
		t.Error("well-known section should be omitted when the map is empty")
	}
}

func TestEnvListTemplatesIncludesWellKnownRepos(t *testing.T) {
	restore := withWellKnownRepo(t, "demo-repo", "https://example.com/demo.git")
	defer restore()

	s, _ := newTestServer(t)

	res, _, err := s.envListTemplates(context.Background(), nil, struct{}{})
	if err != nil {
		t.Fatalf("envListTemplates: %v", err)
	}
	text := textOf(t, res)
	if !strings.Contains(text, "well-known repositories") {
		t.Error("missing the well-known section")
	}
	if !strings.Contains(text, "demo-repo") || !strings.Contains(text, "https://example.com/demo.git") {
		t.Errorf("listing = %q", text)
	}
}
