package main

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

// ---- outputBuffer -----------------------------------------------------------

func TestOutputBufferUnderCap(t *testing.T) {
	b := newOutputBuffer(100)
	n, err := b.Write([]byte("hello"))
	if err != nil {
		t.Fatalf("Write: %v", err)
	}
	if n != 5 {
		t.Errorf("Write returned %d, want 5", n)
	}

	text, truncated, total := b.snapshot()
	if text != "hello" {
		t.Errorf("text = %q, want %q", text, "hello")
	}
	if truncated {
		t.Error("truncated = true, want false")
	}
	if total != 5 {
		t.Errorf("total = %d, want 5", total)
	}
}

func TestOutputBufferExactlyAtCap(t *testing.T) {
	b := newOutputBuffer(5)
	b.Write([]byte("12345"))

	text, truncated, total := b.snapshot()
	if text != "12345" || truncated || total != 5 {
		t.Errorf("got (%q, %v, %d), want (\"12345\", false, 5)", text, truncated, total)
	}
}

func TestOutputBufferRetainsTail(t *testing.T) {
	b := newOutputBuffer(5)
	b.Write([]byte("abcdefghij"))

	text, truncated, total := b.snapshot()
	if text != "fghij" {
		t.Errorf("text = %q, want the last 5 bytes %q", text, "fghij")
	}
	if !truncated {
		t.Error("truncated = false, want true")
	}
	if total != 10 {
		t.Errorf("total = %d, want 10", total)
	}
}

func TestOutputBufferAccumulatesAcrossWrites(t *testing.T) {
	b := newOutputBuffer(4)
	for _, s := range []string{"aa", "bb", "cc"} {
		b.Write([]byte(s))
	}

	text, truncated, total := b.snapshot()
	if text != "bbcc" {
		t.Errorf("text = %q, want %q", text, "bbcc")
	}
	if !truncated {
		t.Error("truncated = false, want true")
	}
	if total != 6 {
		t.Errorf("total = %d, want 6", total)
	}
}

// TestOutputBufferConcurrentWrites exercises the mutex; stdout and stderr are
// written by separate goroutines inside os/exec. Run with -race.
func TestOutputBufferConcurrentWrites(t *testing.T) {
	b := newOutputBuffer(1024)

	var wg sync.WaitGroup
	for i := 0; i < 50; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := 0; j < 20; j++ {
				b.Write([]byte("xy"))
			}
		}()
	}
	wg.Wait()

	_, _, total := b.snapshot()
	if total != 50*20*2 {
		t.Errorf("total = %d, want %d", total, 50*20*2)
	}
}

func TestRenderOutput(t *testing.T) {
	t.Run("untruncated output passes through", func(t *testing.T) {
		b := newOutputBuffer(100)
		b.Write([]byte("build ok\n"))
		if got := renderOutput(b); got != "build ok\n" {
			t.Errorf("got %q, want %q", got, "build ok\n")
		}
	})

	t.Run("truncated output is announced", func(t *testing.T) {
		b := newOutputBuffer(4)
		b.Write([]byte("0123456789"))
		got := renderOutput(b)
		if !strings.HasPrefix(got, "[output truncated: showing last 4 of 10 bytes]\n") {
			t.Errorf("missing truncation notice, got %q", got)
		}
		if !strings.HasSuffix(got, "6789") {
			t.Errorf("tail not preserved, got %q", got)
		}
	})
}

// ---- pure helpers -----------------------------------------------------------

func TestShellQuote(t *testing.T) {
	tests := []struct {
		in   string
		want string
	}{
		{"/workspace/main.go", `'/workspace/main.go'`},
		{"", `''`},
		{"with space", `'with space'`},
		{"it's", `'it'\''s'`},
		{`'`, `''\'''`},
		{"a'b'c", `'a'\''b'\''c'`},
		{`$(whoami)`, `'$(whoami)'`},
		{"back\\slash", `'back\slash'`},
	}

	for _, tt := range tests {
		t.Run(tt.in, func(t *testing.T) {
			if got := shellQuote(tt.in); got != tt.want {
				t.Errorf("shellQuote(%q) = %s, want %s", tt.in, got, tt.want)
			}
		})
	}
}

func TestParseLabels(t *testing.T) {
	tests := []struct {
		name string
		in   string
		want map[string]string
	}{
		{
			name: "podman map rendering",
			in:   "map[devenv-mcp:true devenv-mcp-env-id:env-3]",
			want: map[string]string{"devenv-mcp": "true", "devenv-mcp-env-id": "env-3"},
		},
		{
			name: "docker-style comma separated",
			in:   "devenv-mcp=true,devenv-mcp-env-id=env-3",
			want: map[string]string{"devenv-mcp": "true", "devenv-mcp-env-id": "env-3"},
		},
		{
			name: "empty map",
			in:   "map[]",
			want: map[string]string{},
		},
		{
			name: "empty string",
			in:   "",
			want: map[string]string{},
		},
		{
			name: "entries without separators are dropped",
			in:   "novalue,k=v",
			want: map[string]string{"k": "v"},
		},
		{
			name: "surrounding spaces are trimmed",
			in:   "a=1, b=2",
			want: map[string]string{"a": "1", "b": "2"},
		},
		{
			name: "single map entry",
			in:   "map[only:one]",
			want: map[string]string{"only": "one"},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := parseLabels(tt.in)
			if len(got) != len(tt.want) {
				t.Fatalf("parseLabels(%q) = %v, want %v", tt.in, got, tt.want)
			}
			for k, v := range tt.want {
				if got[k] != v {
					t.Errorf("label %q = %q, want %q", k, got[k], v)
				}
			}
		})
	}
}

func TestPodmanPathDefaultsToPodman(t *testing.T) {
	if got := (&PodmanProvider{}).podman(); got != "podman" {
		t.Errorf("podman() = %q, want %q", got, "podman")
	}
	if got := (&PodmanProvider{PodmanPath: "/usr/bin/podman"}).podman(); got != "/usr/bin/podman" {
		t.Errorf("podman() = %q, want the configured path", got)
	}
}

// ---- PodmanProvider against a stub binary -----------------------------------

// podmanStub is a PodmanProvider pointed at a generated shell script standing
// in for the podman CLI. The script logs every invocation and then runs the
// supplied body, so tests can assert on arguments and drive exit codes.
type podmanStub struct {
	*PodmanProvider
	dir     string
	logPath string
}

func newPodmanStub(t *testing.T, body string) *podmanStub {
	t.Helper()
	dir := t.TempDir()
	logPath := filepath.Join(dir, "calls.log")
	binPath := filepath.Join(dir, "podman")

	// Each invocation appends one tab-separated line of arguments.
	script := "#!/bin/sh\n" +
		"printf '%s\\t' \"$@\" >> " + shellQuote(logPath) + "\n" +
		"printf '\\n' >> " + shellQuote(logPath) + "\n" +
		body + "\n"

	if err := os.WriteFile(binPath, []byte(script), 0o755); err != nil {
		t.Fatalf("write stub: %v", err)
	}
	return &podmanStub{
		PodmanProvider: &PodmanProvider{PodmanPath: binPath},
		dir:            dir,
		logPath:        logPath,
	}
}

// calls returns the recorded invocations, each as its argument slice.
func (s *podmanStub) calls(t *testing.T) [][]string {
	t.Helper()
	data, err := os.ReadFile(s.logPath)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		t.Fatalf("read stub log: %v", err)
	}

	var out [][]string
	for _, line := range strings.Split(strings.TrimRight(string(data), "\n"), "\n") {
		if line == "" {
			continue
		}
		out = append(out, strings.Split(strings.TrimRight(line, "\t"), "\t"))
	}
	return out
}

// onlyCall asserts exactly one invocation was recorded and returns its args.
func (s *podmanStub) onlyCall(t *testing.T) []string {
	t.Helper()
	calls := s.calls(t)
	if len(calls) != 1 {
		t.Fatalf("got %d podman calls, want 1: %v", len(calls), calls)
	}
	return calls[0]
}

func argsEqual(got, want []string) bool {
	if len(got) != len(want) {
		return false
	}
	for i := range got {
		if got[i] != want[i] {
			return false
		}
	}
	return true
}

func containsArgPair(args []string, flag, value string) bool {
	for i := 0; i < len(args)-1; i++ {
		if args[i] == flag && args[i+1] == value {
			return true
		}
	}
	return false
}

func TestPodmanEnsureVolumeAlreadyExists(t *testing.T) {
	s := newPodmanStub(t, "exit 0")

	if err := s.EnsureVolume(context.Background(), "vol"); err != nil {
		t.Fatalf("EnsureVolume: %v", err)
	}

	got := s.onlyCall(t)
	if !argsEqual(got, []string{"volume", "exists", "vol"}) {
		t.Errorf("args = %v, want [volume exists vol]; create should be skipped", got)
	}
}

func TestPodmanEnsureVolumeCreatesWhenMissing(t *testing.T) {
	s := newPodmanStub(t, `if [ "$2" = "exists" ]; then exit 1; fi; exit 0`)

	if err := s.EnsureVolume(context.Background(), "vol"); err != nil {
		t.Fatalf("EnsureVolume: %v", err)
	}

	calls := s.calls(t)
	if len(calls) != 2 {
		t.Fatalf("got %d calls, want 2 (exists, then create): %v", len(calls), calls)
	}
	if !argsEqual(calls[1], []string{"volume", "create", "vol"}) {
		t.Errorf("second call = %v, want [volume create vol]", calls[1])
	}
}

func TestPodmanEnsureVolumeCreateFails(t *testing.T) {
	s := newPodmanStub(t, `if [ "$2" = "exists" ]; then exit 1; fi; echo "volume in use" >&2; exit 2`)

	err := s.EnsureVolume(context.Background(), "vol")
	if err == nil {
		t.Fatal("EnsureVolume: expected an error")
	}
	if !strings.Contains(err.Error(), "volume in use") {
		t.Errorf("error = %q, want it to carry the stderr text", err)
	}
}

func TestPodmanCreate(t *testing.T) {
	s := newPodmanStub(t, "echo deadbeef")

	id, err := s.Create(context.Background(), CreateOpts{
		Image:   "nixos/nix",
		Name:    "devenv-mcp-env-1",
		Labels:  map[string]string{"devenv-mcp": "true"},
		Env:     map[string]string{"FOO": "bar"},
		Volumes: []VolumeMount{{Source: "devenv-mcp-nix", Target: "/nix"}},
	})
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	if id != "deadbeef" {
		t.Errorf("id = %q, want %q (stdout should be trimmed)", id, "deadbeef")
	}

	args := s.onlyCall(t)
	if args[0] != "create" {
		t.Errorf("args[0] = %q, want %q", args[0], "create")
	}
	if !containsArgPair(args, "--name", "devenv-mcp-env-1") {
		t.Errorf("missing --name in %v", args)
	}
	if !containsArgPair(args, "--label", "devenv-mcp=true") {
		t.Errorf("missing --label in %v", args)
	}
	if !containsArgPair(args, "--env", "FOO=bar") {
		t.Errorf("missing --env in %v", args)
	}
	if !containsArgPair(args, "--volume", "devenv-mcp-nix:/nix") {
		t.Errorf("missing --volume in %v", args)
	}
	// The image and the keep-alive command must be last, in that order.
	tail := args[len(args)-3:]
	if !argsEqual(tail, []string{"nixos/nix", "sleep", "infinity"}) {
		t.Errorf("args tail = %v, want [nixos/nix sleep infinity]", tail)
	}
}

func TestPodmanCreateError(t *testing.T) {
	s := newPodmanStub(t, `echo "name already in use" >&2; exit 125`)

	_, err := s.Create(context.Background(), CreateOpts{Image: "img", Name: "dup"})
	if err == nil {
		t.Fatal("Create: expected an error")
	}
	if !strings.Contains(err.Error(), "podman create") || !strings.Contains(err.Error(), "name already in use") {
		t.Errorf("error = %q, want it to name the operation and carry stderr", err)
	}
}

func TestPodmanStartStopRemove(t *testing.T) {
	tests := []struct {
		name string
		call func(*podmanStub) error
		want []string
	}{
		{"start", func(s *podmanStub) error { return s.Start(context.Background(), "cid") }, []string{"start", "cid"}},
		{"stop", func(s *podmanStub) error { return s.Stop(context.Background(), "cid") }, []string{"stop", "--time", "10", "cid"}},
		{"remove", func(s *podmanStub) error { return s.Remove(context.Background(), "cid") }, []string{"rm", "--force", "cid"}},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := newPodmanStub(t, "exit 0")
			if err := tt.call(s); err != nil {
				t.Fatalf("%s: %v", tt.name, err)
			}
			if got := s.onlyCall(t); !argsEqual(got, tt.want) {
				t.Errorf("args = %v, want %v", got, tt.want)
			}
		})
	}
}

func TestPodmanStartError(t *testing.T) {
	s := newPodmanStub(t, `echo "no such container" >&2; exit 125`)

	err := s.Start(context.Background(), "missing")
	if err == nil {
		t.Fatal("Start: expected an error")
	}
	if !strings.Contains(err.Error(), "podman start") || !strings.Contains(err.Error(), "no such container") {
		t.Errorf("error = %q", err)
	}
}

func TestPodmanList(t *testing.T) {
	s := newPodmanStub(t, `cat <<'EOF'
abc123|devenv-mcp-env-1|Up 2 minutes|map[devenv-mcp:true devenv-mcp-env-id:env-1]
def456|devenv-mcp-env-2|Exited (0) 1 hour ago|devenv-mcp=true,devenv-mcp-env-id=env-2

ghi789|malformed-two-fields
jkl012|no-labels|Created
EOF`)

	got, err := s.List(context.Background(), map[string]string{"devenv-mcp": "true"})
	if err != nil {
		t.Fatalf("List: %v", err)
	}
	if len(got) != 3 {
		t.Fatalf("got %d containers, want 3 (blank and malformed lines skipped): %+v", len(got), got)
	}

	if got[0].ID != "abc123" || got[0].Name != "devenv-mcp-env-1" || got[0].Status != "Up 2 minutes" {
		t.Errorf("first container = %+v", got[0])
	}
	if got[0].Labels["devenv-mcp-env-id"] != "env-1" {
		t.Errorf("podman-style labels not parsed: %v", got[0].Labels)
	}
	if got[1].Labels["devenv-mcp-env-id"] != "env-2" {
		t.Errorf("docker-style labels not parsed: %v", got[1].Labels)
	}
	if len(got[2].Labels) != 0 {
		t.Errorf("third container should have no labels, got %v", got[2].Labels)
	}

	args := s.onlyCall(t)
	if args[0] != "ps" {
		t.Errorf("args[0] = %q, want ps", args[0])
	}
	if !containsArgPair(args, "--filter", "label=devenv-mcp=true") {
		t.Errorf("label filter not passed: %v", args)
	}
}

func TestPodmanListEmpty(t *testing.T) {
	s := newPodmanStub(t, "exit 0")

	got, err := s.List(context.Background(), nil)
	if err != nil {
		t.Fatalf("List: %v", err)
	}
	if len(got) != 0 {
		t.Errorf("got %d containers, want 0", len(got))
	}
}

func TestPodmanListError(t *testing.T) {
	s := newPodmanStub(t, `echo "cannot connect" >&2; exit 125`)

	_, err := s.List(context.Background(), nil)
	if err == nil {
		t.Fatal("List: expected an error")
	}
	if !strings.Contains(err.Error(), "podman ps") || !strings.Contains(err.Error(), "cannot connect") {
		t.Errorf("error = %q", err)
	}
}

func TestPodmanExecCombinesStreamsAndReturnsExitCode(t *testing.T) {
	s := newPodmanStub(t, `echo "on stdout"; echo "on stderr" >&2; exit 7`)

	code, out, err := s.Exec(context.Background(), "cid", []string{"false"}, ExecOpts{})
	if err != nil {
		t.Fatalf("Exec returned an error for a non-zero exit: %v", err)
	}
	if code != 7 {
		t.Errorf("code = %d, want 7", code)
	}
	if !strings.Contains(out, "on stdout") || !strings.Contains(out, "on stderr") {
		t.Errorf("output = %q, want both streams", out)
	}
}

func TestPodmanExecSuccess(t *testing.T) {
	s := newPodmanStub(t, "exit 0")

	code, _, err := s.Exec(context.Background(), "cid", []string{"true"}, ExecOpts{})
	if err != nil {
		t.Fatalf("Exec: %v", err)
	}
	if code != 0 {
		t.Errorf("code = %d, want 0", code)
	}
}

func TestPodmanExecArgs(t *testing.T) {
	s := newPodmanStub(t, "exit 0")

	_, _, err := s.Exec(context.Background(), "cid", []string{"go", "test", "./..."},
		ExecOpts{WorkDir: "/workspace", Env: map[string]string{"CI": "1"}})
	if err != nil {
		t.Fatalf("Exec: %v", err)
	}

	args := s.onlyCall(t)
	if args[0] != "exec" {
		t.Errorf("args[0] = %q, want exec", args[0])
	}
	if !containsArgPair(args, "--workdir", "/workspace") {
		t.Errorf("missing --workdir: %v", args)
	}
	if !containsArgPair(args, "--env", "CI=1") {
		t.Errorf("missing --env: %v", args)
	}
	tail := args[len(args)-4:]
	if !argsEqual(tail, []string{"cid", "go", "test", "./..."}) {
		t.Errorf("args tail = %v, want [cid go test ./...]", tail)
	}
}

func TestPodmanExecOmitsEmptyWorkdir(t *testing.T) {
	s := newPodmanStub(t, "exit 0")

	if _, _, err := s.Exec(context.Background(), "cid", []string{"ls"}, ExecOpts{}); err != nil {
		t.Fatalf("Exec: %v", err)
	}

	for _, a := range s.onlyCall(t) {
		if a == "--workdir" {
			t.Error("--workdir should be omitted when WorkDir is empty")
		}
	}
}

func TestPodmanExecForwardsStdin(t *testing.T) {
	s := newPodmanStub(t, "cat")

	_, out, err := s.Exec(context.Background(), "cid", []string{"cat"},
		ExecOpts{Stdin: strings.NewReader("piped input")})
	if err != nil {
		t.Fatalf("Exec: %v", err)
	}
	if out != "piped input" {
		t.Errorf("output = %q, want the stdin content echoed back", out)
	}
}

func TestPodmanExecTruncatesLargeOutput(t *testing.T) {
	// 300 lines of 1000 digits each ≈ 300KB, comfortably over the 256KiB cap.
	s := newPodmanStub(t, `i=0; while [ $i -lt 300 ]; do printf '%01000d\n' $i; i=$((i+1)); done`)

	_, out, err := s.Exec(context.Background(), "cid", []string{"noisy"}, ExecOpts{})
	if err != nil {
		t.Fatalf("Exec: %v", err)
	}
	if !strings.HasPrefix(out, "[output truncated:") {
		t.Errorf("large output was not truncated, got %d bytes starting with %.60q", len(out), out)
	}
	// The notice plus the capped tail; allow a little slack for the header.
	if len(out) > maxExecOutput+200 {
		t.Errorf("output is %d bytes, want at most ~%d", len(out), maxExecOutput)
	}
	if !strings.HasSuffix(out, "\n") {
		t.Error("expected the retained tail to end with the final newline")
	}
}

func TestPodmanExecBinaryMissing(t *testing.T) {
	p := &PodmanProvider{PodmanPath: filepath.Join(t.TempDir(), "does-not-exist")}

	code, _, err := p.Exec(context.Background(), "cid", []string{"ls"}, ExecOpts{})
	if err == nil {
		t.Fatal("Exec: expected an error when the binary is missing")
	}
	if code != -1 {
		t.Errorf("code = %d, want -1 for a non-exit failure", code)
	}
}

// execPassthrough makes the stub run the command podman was asked to run,
// dropping the leading `exec [-i] <container-id>` arguments. This exercises
// WriteFile and ReadFile against a real shell.
const execPassthroughWriteFile = `shift 3; exec "$@"`
const execPassthroughReadFile = `shift 2; exec "$@"`

func TestPodmanWriteFile(t *testing.T) {
	s := newPodmanStub(t, execPassthroughWriteFile)
	target := filepath.Join(s.dir, "nested", "deep", "file.txt")

	if err := s.WriteFile(context.Background(), "cid", target, []byte("hello world")); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}

	got, err := os.ReadFile(target)
	if err != nil {
		t.Fatalf("target not written: %v", err)
	}
	if string(got) != "hello world" {
		t.Errorf("content = %q, want %q", got, "hello world")
	}
}

func TestPodmanWriteFileEmptyContent(t *testing.T) {
	s := newPodmanStub(t, execPassthroughWriteFile)
	target := filepath.Join(s.dir, "empty.txt")

	if err := s.WriteFile(context.Background(), "cid", target, nil); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	got, err := os.ReadFile(target)
	if err != nil {
		t.Fatalf("target not written: %v", err)
	}
	if len(got) != 0 {
		t.Errorf("content = %q, want empty", got)
	}
}

// TestPodmanWriteFileQuotedPath covers paths that would terminate the shell
// string literal the command is built from.
func TestPodmanWriteFileQuotedPath(t *testing.T) {
	s := newPodmanStub(t, execPassthroughWriteFile)
	target := filepath.Join(s.dir, "it's a file.txt")

	if err := s.WriteFile(context.Background(), "cid", target, []byte("quoted")); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}

	got, err := os.ReadFile(target)
	if err != nil {
		t.Fatalf("file with a quote in its name was not written: %v", err)
	}
	if string(got) != "quoted" {
		t.Errorf("content = %q, want %q", got, "quoted")
	}
}

// TestPodmanWriteFileRejectsInjection is the regression test for command
// injection through the file path: the payload must land as a literal filename
// and must not execute.
func TestPodmanWriteFileRejectsInjection(t *testing.T) {
	s := newPodmanStub(t, execPassthroughWriteFile)
	canary := filepath.Join(s.dir, "pwned")
	payload := filepath.Join(s.dir, "x'; touch "+canary+"; echo '")

	if err := s.WriteFile(context.Background(), "cid", payload, []byte("data")); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}

	if _, err := os.Stat(canary); err == nil {
		t.Fatal("command injection: the payload in the path was executed")
	}
	got, err := os.ReadFile(payload)
	if err != nil {
		t.Fatalf("payload should have been treated as a literal filename: %v", err)
	}
	if string(got) != "data" {
		t.Errorf("content = %q, want %q", got, "data")
	}
}

func TestPodmanWriteFileError(t *testing.T) {
	s := newPodmanStub(t, `echo "permission denied" >&2; exit 1`)

	err := s.WriteFile(context.Background(), "cid", "/root/x", []byte("x"))
	if err == nil {
		t.Fatal("WriteFile: expected an error")
	}
	if !strings.Contains(err.Error(), "write file") || !strings.Contains(err.Error(), "permission denied") {
		t.Errorf("error = %q", err)
	}
}

func TestPodmanReadFile(t *testing.T) {
	s := newPodmanStub(t, execPassthroughReadFile)
	target := filepath.Join(s.dir, "read-me.txt")
	if err := os.WriteFile(target, []byte("file contents\n"), 0o644); err != nil {
		t.Fatalf("setup: %v", err)
	}

	got, err := s.ReadFile(context.Background(), "cid", target)
	if err != nil {
		t.Fatalf("ReadFile: %v", err)
	}
	if string(got) != "file contents\n" {
		t.Errorf("content = %q, want %q", got, "file contents\n")
	}

	args := s.onlyCall(t)
	if !argsEqual(args, []string{"exec", "cid", "cat", target}) {
		t.Errorf("args = %v", args)
	}
}

func TestPodmanReadFileError(t *testing.T) {
	s := newPodmanStub(t, `echo "no such file" >&2; exit 1`)

	_, err := s.ReadFile(context.Background(), "cid", "/nope")
	if err == nil {
		t.Fatal("ReadFile: expected an error")
	}
	if !strings.Contains(err.Error(), "read file") || !strings.Contains(err.Error(), "no such file") {
		t.Errorf("error = %q", err)
	}
}

func TestPodmanIsRunning(t *testing.T) {
	tests := []struct {
		name string
		body string
		want bool
	}{
		{"running", "echo true", true},
		{"stopped", "echo false", false},
		{"trailing whitespace is trimmed", `printf 'true\n\n'`, true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := newPodmanStub(t, tt.body)
			got, err := s.IsRunning(context.Background(), "cid")
			if err != nil {
				t.Fatalf("IsRunning: %v", err)
			}
			if got != tt.want {
				t.Errorf("IsRunning = %v, want %v", got, tt.want)
			}
		})
	}
}

func TestPodmanIsRunningArgs(t *testing.T) {
	s := newPodmanStub(t, "echo true")

	if _, err := s.IsRunning(context.Background(), "cid"); err != nil {
		t.Fatalf("IsRunning: %v", err)
	}
	if got := s.onlyCall(t); !argsEqual(got, []string{"inspect", "--format", "{{.State.Running}}", "cid"}) {
		t.Errorf("args = %v", got)
	}
}

func TestPodmanIsRunningError(t *testing.T) {
	s := newPodmanStub(t, `echo "no such container" >&2; exit 125`)

	got, err := s.IsRunning(context.Background(), "gone")
	if err == nil {
		t.Fatal("IsRunning: expected an error")
	}
	if got {
		t.Error("IsRunning = true on error, want false")
	}
	if !strings.Contains(err.Error(), "inspect") {
		t.Errorf("error = %q", err)
	}
}
