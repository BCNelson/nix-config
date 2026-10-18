package guard

import (
	"encoding/json"
	"fmt"
	"io"
	"strings"
)

// Payload is the union of what the agents send to a pre-tool hook:
//
//	Claude Code / Codex: {"tool_name": "Bash", "tool_input": {"command": "..."}}
//	opencode plugin:     {"tool_name": "bash", "tool_input": {"command": "..."}}  (our plugin's shape)
//	pi extension:        {"tool_name": "bash", "tool_input": {"command": "..."}}  (our extension's shape)
//
// Codex may send the command as an argv array; file tools send a path under
// file_path (Claude), filePath (opencode) or path.
type Payload struct {
	ToolName  string         `json:"tool_name"`
	ToolInput map[string]any `json:"tool_input"`
}

// CheckPayload decides on one hook payload.
func CheckPayload(p Payload) Decision {
	tool := strings.ToLower(p.ToolName)
	in := p.ToolInput

	// The ssh MCP server runs commands remotely: inspect them as such.
	if strings.Contains(tool, "ssh_exec") || strings.Contains(tool, "ssh_start") {
		if s, ok := in["command"].(string); ok {
			return checkScript(s, true, 0)
		}
		return allow
	}

	switch v := firstOf(in, "command", "cmd").(type) {
	case string:
		return CheckCommand(v)
	case []any:
		argv := make([]string, 0, len(v))
		for _, a := range v {
			s, _ := a.(string)
			argv = append(argv, s)
		}
		return CheckArgv(argv)
	}

	// File tools. Glob only lists names, which is harmless.
	if tool == "glob" {
		return allow
	}
	if s, ok := firstOf(in, "file_path", "filePath", "path").(string); ok {
		return CheckReadPath(s)
	}
	return allow
}

func firstOf(m map[string]any, keys ...string) any {
	for _, k := range keys {
		if v, ok := m[k]; ok && v != nil {
			return v
		}
	}
	return nil
}

// RunHook reads one payload from r and writes the agent-specific answer.
// It returns the process exit code.
//
//	claude, codex: exit 0 with hookSpecificOutput.permissionDecision = "deny"
//	               (both agents share Claude's PreToolUse schema)
//	opencode, pi, plain: a JSON Decision on stdout; exit 2 when blocked
func RunHook(agent string, r io.Reader, stdout, stderr io.Writer) int {
	raw, err := io.ReadAll(r)
	if err != nil {
		fmt.Fprintln(stderr, "agent-guard: reading payload:", err)
		return 2
	}
	var p Payload
	if err := json.Unmarshal(raw, &p); err != nil {
		// Not JSON: treat the input as a bare command line (handy for tests
		// and for agents that pass the command as text).
		p = Payload{ToolName: "bash", ToolInput: map[string]any{"command": strings.TrimSpace(string(raw))}}
	}
	d := CheckPayload(p)

	switch agent {
	case "claude", "codex":
		if !d.Block {
			return 0
		}
		out := map[string]any{
			"hookSpecificOutput": map[string]any{
				"hookEventName":            "PreToolUse",
				"permissionDecision":       "deny",
				"permissionDecisionReason": d.Message(),
			},
		}
		_ = json.NewEncoder(stdout).Encode(out)
		return 0
	default:
		out := struct {
			Decision
			Message string `json:"message,omitempty"`
		}{d, d.Message()}
		_ = json.NewEncoder(stdout).Encode(out)
		if d.Block {
			fmt.Fprintln(stderr, d.Message())
			return 2
		}
		return 0
	}
}
