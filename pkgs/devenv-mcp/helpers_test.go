package main

import (
	"strings"
	"testing"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

func TestParseFlags(t *testing.T) {
	defaults := config{image: "docker.io/nixos/nix:latest", workDir: "/workspace", maxEnvs: 10}

	tests := []struct {
		name string
		args []string
		want config
	}{
		{"no args uses defaults", nil, defaults},
		{
			"all flags set",
			[]string{"--podman-path", "/run/podman", "--image", "alpine", "--work-dir", "/src", "--max-envs", "3"},
			config{podmanPath: "/run/podman", image: "alpine", workDir: "/src", maxEnvs: 3},
		},
		{
			"unknown flags are ignored",
			[]string{"--nope", "--image", "alpine"},
			config{image: "alpine", workDir: "/workspace", maxEnvs: 10},
		},
		{
			"trailing flag without a value keeps the default",
			[]string{"--image"},
			defaults,
		},
		{
			"non-numeric max-envs keeps the default",
			[]string{"--max-envs", "abc"},
			defaults,
		},
		{
			"zero max-envs keeps the default",
			[]string{"--max-envs", "0"},
			defaults,
		},
		{
			"multi-digit max-envs",
			[]string{"--max-envs", "125"},
			config{image: defaults.image, workDir: defaults.workDir, maxEnvs: 125},
		},
		{
			"later flags win",
			[]string{"--work-dir", "/a", "--work-dir", "/b"},
			config{image: defaults.image, workDir: "/b", maxEnvs: 10},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := parseFlags(tt.args); got != tt.want {
				t.Errorf("parseFlags(%q) = %+v, want %+v", tt.args, got, tt.want)
			}
		})
	}
}

func TestResolveWithin(t *testing.T) {
	const root = "/workspace"

	tests := []struct {
		name    string
		path    string
		want    string
		wantErr string
	}{
		{name: "relative path", path: "main.go", want: "/workspace/main.go"},
		{name: "nested relative path", path: "cmd/app/main.go", want: "/workspace/cmd/app/main.go"},
		{name: "absolute path inside root", path: "/workspace/go.mod", want: "/workspace/go.mod"},
		{name: "the root itself", path: "/workspace", want: "/workspace"},
		{name: "trailing slash is cleaned", path: "/workspace/sub/", want: "/workspace/sub"},
		{name: "interior dot-dot that stays inside", path: "sub/../main.go", want: "/workspace/main.go"},
		{name: "redundant separators", path: "//workspace//a///b", want: "/workspace/a/b"},

		{name: "empty path", path: "", wantErr: "path is required"},
		{name: "absolute escape", path: "/etc/shadow", wantErr: "must be under /workspace"},
		{name: "dot-dot escape", path: "/workspace/../etc/shadow", wantErr: "must be under /workspace"},
		{name: "relative dot-dot escape", path: "../etc/shadow", wantErr: "must be under /workspace"},
		{name: "deep dot-dot escape", path: "a/b/../../../etc/shadow", wantErr: "must be under /workspace"},
		{name: "sibling with a shared prefix", path: "/workspace-evil/x", wantErr: "must be under /workspace"},
		{name: "parent of root", path: "/", wantErr: "must be under /workspace"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := resolveWithin(root, tt.path)
			if tt.wantErr != "" {
				if err == nil {
					t.Fatalf("resolveWithin(%q, %q) = %q, want error containing %q", root, tt.path, got, tt.wantErr)
				}
				if !strings.Contains(err.Error(), tt.wantErr) {
					t.Fatalf("error = %q, want it to contain %q", err, tt.wantErr)
				}
				return
			}
			if err != nil {
				t.Fatalf("resolveWithin(%q, %q): unexpected error %v", root, tt.path, err)
			}
			if got != tt.want {
				t.Errorf("resolveWithin(%q, %q) = %q, want %q", root, tt.path, got, tt.want)
			}
		})
	}
}

func TestResolveWithinUncleanRoot(t *testing.T) {
	got, err := resolveWithin("/workspace/", "main.go")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if got != "/workspace/main.go" {
		t.Errorf("got %q, want /workspace/main.go", got)
	}
}

func TestRepoName(t *testing.T) {
	tests := []struct {
		url  string
		want string
	}{
		{"https://github.com/owner/repo.git", "repo"},
		{"https://github.com/owner/repo", "repo"},
		{"git@github.com:owner/repo.git", "repo"},
		{"repo", "repo"},
		{"", ""},
		{"https://example.com/a/b/c.git", "c"},
		{"https://example.com/trailing/", ""},
	}

	for _, tt := range tests {
		t.Run(tt.url, func(t *testing.T) {
			if got := repoName(tt.url); got != tt.want {
				t.Errorf("repoName(%q) = %q, want %q", tt.url, got, tt.want)
			}
		})
	}
}

func TestValidateTemplate(t *testing.T) {
	tests := []struct {
		name    string
		tmpl    string
		gitURL  string
		wantErr string
	}{
		{name: "empty template is allowed", tmpl: ""},
		{name: "builtin template", tmpl: "go"},
		{name: "every other builtin", tmpl: "node-pnpm"},
		{name: "auto with a repo", tmpl: "auto", gitURL: "https://example.com/r.git"},
		{name: "auto without a repo", tmpl: "auto", wantErr: "requires git_url"},
		{name: "unknown template", tmpl: "haskell", wantErr: `unknown template "haskell"`},
		{name: "unknown template lists the options", tmpl: "haskell", wantErr: "go, rust, python"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := validateTemplate(tt.tmpl, tt.gitURL)
			if tt.wantErr == "" {
				if err != nil {
					t.Fatalf("validateTemplate(%q, %q): unexpected error %v", tt.tmpl, tt.gitURL, err)
				}
				return
			}
			if err == nil || !strings.Contains(err.Error(), tt.wantErr) {
				t.Fatalf("error = %v, want it to contain %q", err, tt.wantErr)
			}
		})
	}
}

func TestValidateTemplateAcceptsWellKnownRepo(t *testing.T) {
	restore := withWellKnownRepo(t, "my-template", "https://example.com/my-template.git")
	defer restore()

	if err := validateTemplate("my-template", ""); err != nil {
		t.Errorf("well-known repo should validate without a git_url, got %v", err)
	}
}

func TestTextResult(t *testing.T) {
	res := textResult("hello")
	if len(res.Content) != 1 {
		t.Fatalf("got %d content blocks, want 1", len(res.Content))
	}
	tc, ok := res.Content[0].(*mcp.TextContent)
	if !ok {
		t.Fatalf("content is %T, want *mcp.TextContent", res.Content[0])
	}
	if tc.Text != "hello" {
		t.Errorf("text = %q, want %q", tc.Text, "hello")
	}
}

func TestRenderEnvList(t *testing.T) {
	t.Run("empty", func(t *testing.T) {
		if got := renderEnvList(envListOutput{}); got != "no environments" {
			t.Errorf("got %q, want %q", got, "no environments")
		}
	})

	t.Run("all fields", func(t *testing.T) {
		got := renderEnvList(envListOutput{Environments: []envInfoOutput{
			{EnvID: "env-1", Name: "repo", Status: "ready", Template: "go", RepoURL: "https://example.com/r.git"},
			{EnvID: "env-2", Name: "bare", Status: "error", Error: "boom"},
		}})
		want := "env-1 name=repo status=ready template=go repo=https://example.com/r.git\n" +
			`env-2 name=bare status=error error="boom"`
		if got != want {
			t.Errorf("got:\n%s\nwant:\n%s", got, want)
		}
	})

	t.Run("optional fields are omitted", func(t *testing.T) {
		got := renderEnvList(envListOutput{Environments: []envInfoOutput{
			{EnvID: "env-1", Name: "x", Status: "creating"},
		}})
		if got != "env-1 name=x status=creating" {
			t.Errorf("got %q", got)
		}
	})
}

func TestRenderExec(t *testing.T) {
	code := 0
	t.Run("successful exit", func(t *testing.T) {
		got := renderExec(envExecOutput{EnvID: "env-1", Status: "exited", ExitCode: &code, ElapsedSec: 1.24, Output: "hi\n"})
		want := "env=env-1 status=exited exit=0 elapsed=1.2s\nhi\n"
		if got != want {
			t.Errorf("got %q, want %q", got, want)
		}
	})

	t.Run("timeout has no exit code and reports the error", func(t *testing.T) {
		got := renderExec(envExecOutput{EnvID: "env-1", Status: "timeout", ElapsedSec: 300, Error: "timed out after 5m0s", Output: "partial"})
		want := "env=env-1 status=timeout elapsed=300.0s\nerror: timed out after 5m0s\npartial"
		if got != want {
			t.Errorf("got %q, want %q", got, want)
		}
	})
}

// withWellKnownRepo temporarily registers a well-known repo. The map is a
// package-level var, so tests that touch it must not run in parallel.
func withWellKnownRepo(t *testing.T, name, url string) func() {
	t.Helper()
	prev, existed := wellKnownRepos[name]
	wellKnownRepos[name] = url
	return func() {
		if existed {
			wellKnownRepos[name] = prev
			return
		}
		delete(wellKnownRepos, name)
	}
}
