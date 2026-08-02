package main

import (
	"context"
	"encoding/json"
	"strings"
	"testing"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

func TestNewSrvWiring(t *testing.T) {
	s := newSrv(config{podmanPath: "/custom/podman", image: "img", workDir: "/src", maxEnvs: 4})

	provider, ok := s.provider.(*PodmanProvider)
	if !ok {
		t.Fatalf("provider is %T, want *PodmanProvider", s.provider)
	}
	if provider.PodmanPath != "/custom/podman" {
		t.Errorf("PodmanPath = %q", provider.PodmanPath)
	}
	if s.registry.image != "img" || s.registry.workDir != "/src" || s.registry.maxEnvs != 4 {
		t.Errorf("registry = %+v, want the config applied", s.registry)
	}
	if s.registry.provider != s.provider {
		t.Error("the registry and the server must share one provider")
	}
}

// connect wires an in-memory client to the real MCP server so tests exercise
// tool registration, schema generation, and dispatch end to end.
func connect(t *testing.T, s *srv) *mcp.ClientSession {
	t.Helper()
	ctx := context.Background()

	clientTransport, serverTransport := mcp.NewInMemoryTransports()
	serverSession, err := newMCPServer(s).Connect(ctx, serverTransport, nil)
	if err != nil {
		t.Fatalf("server connect: %v", err)
	}
	t.Cleanup(func() { serverSession.Close() })

	client := mcp.NewClient(&mcp.Implementation{Name: "test-client", Version: "0.0.1"}, nil)
	clientSession, err := client.Connect(ctx, clientTransport, nil)
	if err != nil {
		t.Fatalf("client connect: %v", err)
	}
	t.Cleanup(func() { clientSession.Close() })

	return clientSession
}

func TestServerRegistersAllTools(t *testing.T) {
	s, _ := newTestServer(t)
	cs := connect(t, s)

	res, err := cs.ListTools(context.Background(), nil)
	if err != nil {
		t.Fatalf("ListTools: %v", err)
	}

	got := map[string]*mcp.Tool{}
	for _, tool := range res.Tools {
		got[tool.Name] = tool
	}

	want := []string{
		"env_create", "env_list", "env_destroy", "env_exec",
		"env_read_file", "env_write_file", "env_list_dir", "env_list_templates",
		"env_wait", "env_job_list", "env_job_output", "env_job_kill",
	}
	for _, name := range want {
		tool, ok := got[name]
		if !ok {
			t.Errorf("tool %q is not registered", name)
			continue
		}
		if tool.Description == "" {
			t.Errorf("tool %q has no description", name)
		}
		if tool.InputSchema == nil {
			t.Errorf("tool %q has no input schema", name)
		}
	}
	if len(res.Tools) != len(want) {
		t.Errorf("got %d tools, want %d: %v", len(res.Tools), len(want), got)
	}
}

// TestServerToolSchemas checks that the jsonschema struct tags produce the
// parameters agents are told to send.
func TestServerToolSchemas(t *testing.T) {
	s, _ := newTestServer(t)
	cs := connect(t, s)

	res, err := cs.ListTools(context.Background(), nil)
	if err != nil {
		t.Fatalf("ListTools: %v", err)
	}

	want := map[string][]string{
		"env_create":     {"template", "git_url", "name"},
		"env_exec":       {"env_id", "command", "work_dir", "timeout_sec", "background"},
		"env_write_file": {"env_id", "path", "content"},
		"env_destroy":    {"env_id"},
		"env_wait":       {"env_id", "condition", "job_id", "command", "port", "path", "pattern", "timeout_sec"},
		"env_job_output": {"job_id", "tail"},
		"env_job_kill":   {"job_id", "force"},
	}

	for _, tool := range res.Tools {
		props, ok := want[tool.Name]
		if !ok {
			continue
		}
		schema := schemaProperties(t, tool)
		for _, prop := range props {
			if _, ok := schema[prop]; !ok {
				t.Errorf("tool %q is missing the %q property (have %v)", tool.Name, prop, schema)
			}
		}
	}
}

// schemaProperties reads the property names out of a tool's generated input
// schema, which the SDK exposes as an untyped value.
func schemaProperties(t *testing.T, tool *mcp.Tool) map[string]any {
	t.Helper()
	raw, err := json.Marshal(tool.InputSchema)
	if err != nil {
		t.Fatalf("marshal %s schema: %v", tool.Name, err)
	}
	var schema struct {
		Properties map[string]any `json:"properties"`
	}
	if err := json.Unmarshal(raw, &schema); err != nil {
		t.Fatalf("unmarshal %s schema: %v", tool.Name, err)
	}
	return schema.Properties
}

func TestServerCallTool(t *testing.T) {
	s, _ := newTestServer(t)
	cs := connect(t, s)
	ctx := context.Background()

	// A tool with no arguments.
	res, err := cs.CallTool(ctx, &mcp.CallToolParams{Name: "env_list_templates"})
	if err != nil {
		t.Fatalf("CallTool: %v", err)
	}
	if res.IsError {
		t.Fatalf("env_list_templates reported an error: %+v", res.Content)
	}
	if !strings.Contains(textOf(t, res), "go") {
		t.Error("template listing did not come through the transport")
	}

	// A tool with arguments, then observe the state change via another tool.
	res, err = cs.CallTool(ctx, &mcp.CallToolParams{
		Name:      "env_create",
		Arguments: map[string]any{"template": "rust", "name": "via-protocol"},
	})
	if err != nil {
		t.Fatalf("CallTool: %v", err)
	}
	if res.IsError {
		t.Fatalf("env_create reported an error: %+v", res.Content)
	}

	env, ok := s.registry.Get("env-1")
	if !ok {
		t.Fatal("env_create did not register an environment")
	}
	waitBootstrap(t, env)

	res, err = cs.CallTool(ctx, &mcp.CallToolParams{Name: "env_list"})
	if err != nil {
		t.Fatalf("CallTool: %v", err)
	}
	listing := textOf(t, res)
	if !strings.Contains(listing, "via-protocol") || !strings.Contains(listing, "template=rust") {
		t.Errorf("env_list = %q", listing)
	}
}

// TestServerCallToolError checks that handler errors reach the client as tool
// errors rather than transport failures, so an agent can read and recover.
func TestServerCallToolError(t *testing.T) {
	s, _ := newTestServer(t)
	cs := connect(t, s)

	res, err := cs.CallTool(context.Background(), &mcp.CallToolParams{
		Name:      "env_create",
		Arguments: map[string]any{"template": "haskell"},
	})
	if err != nil {
		t.Fatalf("CallTool returned a transport error: %v", err)
	}
	if !res.IsError {
		t.Fatal("expected an unknown template to be reported as a tool error")
	}
	if !strings.Contains(textOf(t, res), "unknown template") {
		t.Errorf("error content = %+v", res.Content)
	}
}

func TestServerCallUnknownTool(t *testing.T) {
	s, _ := newTestServer(t)
	cs := connect(t, s)

	_, err := cs.CallTool(context.Background(), &mcp.CallToolParams{Name: "env_teleport"})
	if err == nil {
		t.Fatal("expected an error for an unregistered tool")
	}
}
