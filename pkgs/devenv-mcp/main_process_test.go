package main

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// mainProcessEnv marks the re-executed test binary as the server subprocess,
// and mainProcessArgs carries the server's flags. The flags travel in the
// environment because the testing package parses the real command line and
// would reject them.
const (
	mainProcessEnv  = "DEVENV_MCP_RUN_MAIN"
	mainProcessArgs = "DEVENV_MCP_MAIN_ARGS"
)

// TestMainServesOverStdio runs the real entrypoint as a subprocess and speaks
// MCP to it over stdin/stdout. Nothing else covers main() — tool registration
// panics surface only when the process actually starts, which is precisely the
// failure this guards against.
func TestMainServesOverStdio(t *testing.T) {
	if os.Getenv(mainProcessEnv) == "1" {
		// Re-executed as the server: hand control to the real entrypoint.
		os.Args = append([]string{"devenv-mcp"}, strings.Fields(os.Getenv(mainProcessArgs))...)
		main()
		return
	}

	// A stub podman on PATH keeps startup reconciliation from shelling out to
	// a real container runtime.
	dir := t.TempDir()
	stub := filepath.Join(dir, "podman")
	if err := os.WriteFile(stub, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatalf("write podman stub: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	cmd := exec.Command(os.Args[0], "-test.run=TestMainServesOverStdio")
	cmd.Env = append(os.Environ(),
		mainProcessEnv+"=1",
		mainProcessArgs+"=--podman-path "+stub+" --max-envs 2",
	)
	cmd.Stderr = os.Stderr

	client := mcp.NewClient(&mcp.Implementation{Name: "test-client", Version: "0.0.1"}, nil)
	session, err := client.Connect(ctx, &mcp.CommandTransport{Command: cmd}, nil)
	if err != nil {
		t.Fatalf("the server process did not come up: %v", err)
	}
	defer session.Close()

	res, err := session.ListTools(ctx, nil)
	if err != nil {
		t.Fatalf("ListTools over stdio: %v", err)
	}
	if len(res.Tools) != 12 {
		t.Errorf("got %d tools over stdio, want 12", len(res.Tools))
	}

	// Exercise a full request/response round trip through the real binary.
	out, err := session.CallTool(ctx, &mcp.CallToolParams{Name: "env_list"})
	if err != nil {
		t.Fatalf("CallTool over stdio: %v", err)
	}
	if out.IsError {
		t.Fatalf("env_list reported an error: %+v", out.Content)
	}
	if got := textOf(t, out); got != "no environments" {
		t.Errorf("env_list = %q, want %q", got, "no environments")
	}

	// Flags must reach the running server: --max-envs 2 should be enforced.
	for i := 0; i < 2; i++ {
		if _, err := session.CallTool(ctx, &mcp.CallToolParams{
			Name:      "env_create",
			Arguments: map[string]any{"template": "go"},
		}); err != nil {
			t.Fatalf("env_create %d: %v", i, err)
		}
	}
	third, err := session.CallTool(ctx, &mcp.CallToolParams{
		Name:      "env_create",
		Arguments: map[string]any{"template": "go"},
	})
	if err != nil {
		t.Fatalf("third env_create: %v", err)
	}
	if !third.IsError || !strings.Contains(textOf(t, third), "maximum 2 environments") {
		t.Errorf("--max-envs was not applied; result = %+v", third.Content)
	}
}
