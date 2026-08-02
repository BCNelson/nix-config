package main

import (
	"context"
	"errors"
	"fmt"
	"regexp"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// procNetTCP renders a /proc/net/tcp table with one socket per entry.
func procNetTCP(entries ...string) string {
	header := "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n"
	return header + strings.Join(entries, "\n") + "\n"
}

// listenEntry is a LISTEN socket on the given port, bound to 127.0.0.1.
func listenEntry(port int) string {
	return fmt.Sprintf("   0: 0100007F:%04X 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1 1 0 100 0 0 10 0", port)
}

// establishedEntry is a connection to the given port, which is not a listener.
func establishedEntry(port int) string {
	return fmt.Sprintf("   1: 0100007F:%04X 0100007F:B3A2 01 00000000:00000000 00:00000000 00000000  1000        0 2 1 0 100 0 0 10 0", port)
}

func TestListeningOn(t *testing.T) {
	tests := []struct {
		name string
		proc string
		port int
		want bool
	}{
		{"listening", procNetTCP(listenEntry(8080)), 8080, true},
		{"different port", procNetTCP(listenEntry(3000)), 8080, false},
		{"connected but not listening", procNetTCP(establishedEntry(8080)), 8080, false},
		{"listener among several sockets", procNetTCP(establishedEntry(443), listenEntry(8080)), 8080, true},
		{"empty table", procNetTCP(), 8080, false},
		{"empty input", "", 8080, false},
		{"header only is not parsed as a socket", "  sl  local_address rem_address   st\n", 8080, false},
		// A low port must not match a high port that shares its hex suffix.
		{"no suffix collision", procNetTCP(listenEntry(0x1F90)), 0x0F90, false},
		{"port 1", procNetTCP(listenEntry(1)), 1, true},
		{"max port", procNetTCP(listenEntry(65535)), 65535, true},
		{"ipv6 style address", procNetTCP("   0: 00000000000000000000000001000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 1 1 0"), 8080, true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := listeningOn(tt.proc, tt.port); got != tt.want {
				t.Errorf("listeningOn(port %d) = %t, want %t", tt.port, got, tt.want)
			}
		})
	}
}

func TestWaitForMetImmediately(t *testing.T) {
	var probes int32
	p := func(context.Context) (bool, string, error) {
		atomic.AddInt32(&probes, 1)
		return true, "ok", nil
	}

	res, err := waitFor(context.Background(), p, time.Second, time.Millisecond)
	if err != nil {
		t.Fatalf("waitFor: %v", err)
	}
	if !res.met || res.detail != "ok" {
		t.Errorf("res = %+v, want met with detail ok", res)
	}
	if got := atomic.LoadInt32(&probes); got != 1 {
		t.Errorf("probed %d times, want exactly 1", got)
	}
}

func TestWaitForMetAfterRetries(t *testing.T) {
	var probes int32
	p := func(context.Context) (bool, string, error) {
		n := atomic.AddInt32(&probes, 1)
		if n < 3 {
			return false, "not yet", nil
		}
		return true, "ready", nil
	}

	res, err := waitFor(context.Background(), p, 5*time.Second, time.Millisecond)
	if err != nil {
		t.Fatalf("waitFor: %v", err)
	}
	if !res.met || res.detail != "ready" {
		t.Errorf("res = %+v, want met with detail ready", res)
	}
	if got := atomic.LoadInt32(&probes); got != 3 {
		t.Errorf("probed %d times, want 3", got)
	}
}

// TestWaitForTimesOutWithLastDetail checks that a timeout is an ordinary
// result carrying the last observation, not an error: the agent needs to see
// why the condition never held.
func TestWaitForTimesOutWithLastDetail(t *testing.T) {
	p := func(context.Context) (bool, string, error) { return false, "still building", nil }

	start := time.Now()
	res, err := waitFor(context.Background(), p, 50*time.Millisecond, time.Millisecond)
	if err != nil {
		t.Fatalf("a timeout must not be an error, got %v", err)
	}
	if res.met {
		t.Error("res.met = true, want false")
	}
	if res.detail != "still building" {
		t.Errorf("detail = %q, want the last observation", res.detail)
	}
	if elapsed := time.Since(start); elapsed < 50*time.Millisecond {
		t.Errorf("returned after %s, want it to wait out the timeout", elapsed)
	}
	if res.waitedSec <= 0 {
		t.Error("waitedSec should be positive")
	}
}

func TestWaitForProbeError(t *testing.T) {
	p := func(context.Context) (bool, string, error) { return false, "", errors.New("container is gone") }

	if _, err := waitFor(context.Background(), p, time.Second, time.Millisecond); err == nil {
		t.Fatal("expected the probe error to abort the wait")
	}
}

func TestWaitForHonoursCancellation(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	p := func(context.Context) (bool, string, error) { return false, "no", nil }

	go func() {
		time.Sleep(10 * time.Millisecond)
		cancel()
	}()

	if _, err := waitFor(ctx, p, time.Minute, time.Millisecond); !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v, want context.Canceled", err)
	}
}

// TestWaitForBacksOff checks that a long wait does not probe at the base
// interval forever: the gap grows toward maxPollInterval.
func TestWaitForBacksOff(t *testing.T) {
	var probes int32
	p := func(context.Context) (bool, string, error) {
		atomic.AddInt32(&probes, 1)
		return false, "no", nil
	}

	if _, err := waitFor(context.Background(), p, 120*time.Millisecond, time.Millisecond); err != nil {
		t.Fatalf("waitFor: %v", err)
	}
	// Without backoff a 1ms base over 120ms would probe ~100 times.
	if got := atomic.LoadInt32(&probes); got > 20 {
		t.Errorf("probed %d times in 120ms, expected backoff to keep it well under 20", got)
	}
}

func TestProbeTimeout(t *testing.T) {
	t.Run("capped at maxProbeTimeout", func(t *testing.T) {
		if got := probeTimeout(time.Now().Add(time.Hour)); got != maxProbeTimeout {
			t.Errorf("got %s, want %s", got, maxProbeTimeout)
		}
	})
	t.Run("shrinks to the remaining time", func(t *testing.T) {
		got := probeTimeout(time.Now().Add(5 * time.Second))
		if got > 5*time.Second || got < 4*time.Second {
			t.Errorf("got %s, want about 5s", got)
		}
	})
	t.Run("floors at one second past the deadline", func(t *testing.T) {
		if got := probeTimeout(time.Now().Add(-time.Minute)); got != time.Second {
			t.Errorf("got %s, want 1s so the final probe still runs", got)
		}
	})
}

func TestWaitForClose(t *testing.T) {
	t.Run("already closed", func(t *testing.T) {
		ch := make(chan struct{})
		close(ch)
		res, err := waitForClose(context.Background(), ch, time.Second)
		if err != nil || !res.met {
			t.Fatalf("res = %+v, err = %v, want met", res, err)
		}
	})

	t.Run("closes during the wait", func(t *testing.T) {
		ch := make(chan struct{})
		go func() {
			time.Sleep(10 * time.Millisecond)
			close(ch)
		}()
		res, err := waitForClose(context.Background(), ch, time.Second)
		if err != nil || !res.met {
			t.Fatalf("res = %+v, err = %v, want met", res, err)
		}
	})

	t.Run("times out", func(t *testing.T) {
		res, err := waitForClose(context.Background(), make(chan struct{}), 20*time.Millisecond)
		if err != nil {
			t.Fatalf("a timeout must not be an error, got %v", err)
		}
		if res.met {
			t.Error("res.met = true, want false")
		}
	})
}

func TestTruncate(t *testing.T) {
	if got := truncate("short", 10); got != "short" {
		t.Errorf("got %q, want it unchanged", got)
	}
	if got := truncate("0123456789", 4); got != "0123…" {
		t.Errorf("got %q, want it cut with a marker", got)
	}
}

// ---- probes ----------------------------------------------------------------

func TestCommandProbe(t *testing.T) {
	f := newFakeProvider()

	t.Run("exit zero is met", func(t *testing.T) {
		f.setExec(func([]string) (int, string, error) { return 0, "up\n", nil })
		met, detail, err := commandProbe(f, "ctr-1", "curl -sf localhost", "/workspace")(context.Background())
		if err != nil || !met {
			t.Fatalf("met = %t, err = %v, want met", met, err)
		}
		if !strings.Contains(detail, "exit=0") || !strings.Contains(detail, "up") {
			t.Errorf("detail = %q, want the exit code and output", detail)
		}
	})

	t.Run("non-zero is not met", func(t *testing.T) {
		f.setExec(func([]string) (int, string, error) { return 7, "", nil })
		met, detail, err := commandProbe(f, "ctr-1", "false", "")(context.Background())
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if met || detail != "exit=7" {
			t.Errorf("met = %t, detail = %q", met, detail)
		}
	})

	t.Run("provider failure aborts", func(t *testing.T) {
		f.setExec(func([]string) (int, string, error) { return 0, "", errors.New("no such container") })
		if _, _, err := commandProbe(f, "ctr-1", "true", "")(context.Background()); err == nil {
			t.Fatal("expected the provider error to surface")
		}
	})
}

func TestFileExistsProbe(t *testing.T) {
	f := newFakeProvider()
	var gotCmd []string
	f.setExec(func(cmd []string) (int, string, error) {
		gotCmd = cmd
		return 0, "", nil
	})

	met, detail, err := fileExistsProbe(f, "ctr-1", "/workspace/target/app")(context.Background())
	if err != nil || !met {
		t.Fatalf("met = %t, err = %v, want met", met, err)
	}
	if !strings.Contains(detail, "/workspace/target/app") {
		t.Errorf("detail = %q", detail)
	}
	if len(gotCmd) != 3 || gotCmd[0] != "test" || gotCmd[1] != "-e" {
		t.Errorf("cmd = %v, want test -e <path>", gotCmd)
	}

	f.setExec(func([]string) (int, string, error) { return 1, "", nil })
	met, detail, _ = fileExistsProbe(f, "ctr-1", "/workspace/nope")(context.Background())
	if met || !strings.Contains(detail, "does not exist") {
		t.Errorf("met = %t, detail = %q", met, detail)
	}
}

func TestFileMatchesProbe(t *testing.T) {
	f := newFakeProvider()
	re := regexp.MustCompile(`Listening on :(\d+)`)

	t.Run("match", func(t *testing.T) {
		f.setExec(func([]string) (int, string, error) {
			return 0, "compiling…\nListening on :8080\n", nil
		})
		met, detail, err := fileMatchesProbe(f, "ctr-1", "/workspace/srv.log", re)(context.Background())
		if err != nil || !met {
			t.Fatalf("met = %t, err = %v, want met", met, err)
		}
		if !strings.Contains(detail, "Listening on :8080") {
			t.Errorf("detail = %q, want the matched text", detail)
		}
	})

	t.Run("no match yet", func(t *testing.T) {
		f.setExec(func([]string) (int, string, error) { return 0, "compiling…\n", nil })
		met, detail, err := fileMatchesProbe(f, "ctr-1", "/workspace/srv.log", re)(context.Background())
		if err != nil || met {
			t.Fatalf("met = %t, err = %v, want not met", met, err)
		}
		if !strings.Contains(detail, "no match") {
			t.Errorf("detail = %q", detail)
		}
	})

	// A log file that does not exist yet is the normal case right after a
	// background job starts, so it must not abort the wait.
	t.Run("missing file is not an error", func(t *testing.T) {
		f.setExec(func([]string) (int, string, error) { return 1, "tail: no such file", nil })
		met, detail, err := fileMatchesProbe(f, "ctr-1", "/workspace/srv.log", re)(context.Background())
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if met || !strings.Contains(detail, "not readable yet") {
			t.Errorf("met = %t, detail = %q", met, detail)
		}
	})

	t.Run("reads a bounded tail", func(t *testing.T) {
		var gotCmd []string
		f.setExec(func(cmd []string) (int, string, error) {
			gotCmd = cmd
			return 0, "", nil
		})
		_, _, _ = fileMatchesProbe(f, "ctr-1", "/workspace/srv.log", re)(context.Background())
		want := []string{"tail", "-c", fmt.Sprint(fileMatchWindow), "/workspace/srv.log"}
		if fmt.Sprint(gotCmd) != fmt.Sprint(want) {
			t.Errorf("cmd = %v, want %v", gotCmd, want)
		}
	})
}

func TestPortOpenProbe(t *testing.T) {
	f := newFakeProvider()

	t.Run("open", func(t *testing.T) {
		f.setExec(func([]string) (int, string, error) { return 0, procNetTCP(listenEntry(8080)), nil })
		met, detail, err := portOpenProbe(f, "ctr-1", 8080)(context.Background())
		if err != nil || !met {
			t.Fatalf("met = %t, err = %v, want met", met, err)
		}
		if !strings.Contains(detail, "8080") {
			t.Errorf("detail = %q", detail)
		}
	})

	t.Run("closed", func(t *testing.T) {
		f.setExec(func([]string) (int, string, error) { return 0, procNetTCP(), nil })
		met, detail, err := portOpenProbe(f, "ctr-1", 8080)(context.Background())
		if err != nil || met {
			t.Fatalf("met = %t, err = %v, want not met", met, err)
		}
		if !strings.Contains(detail, "nothing is listening") {
			t.Errorf("detail = %q", detail)
		}
	})

	t.Run("unreadable proc aborts", func(t *testing.T) {
		f.setExec(func([]string) (int, string, error) { return 1, "", nil })
		if _, _, err := portOpenProbe(f, "ctr-1", 8080)(context.Background()); err == nil {
			t.Fatal("expected an error when /proc/net/tcp cannot be read")
		}
	})
}

// ---- env_wait handler ------------------------------------------------------

func TestEnvWaitEnvReady(t *testing.T) {
	s, _ := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	_, out, err := s.envWait(context.Background(), nil, envWaitInput{EnvID: env.ID, Condition: condEnvReady})
	if err != nil {
		t.Fatalf("envWait: %v", err)
	}
	if !out.Met || out.Status != "met" {
		t.Errorf("out = %+v, want met", out)
	}
	if !strings.Contains(out.Detail, "status=ready") {
		t.Errorf("detail = %q", out.Detail)
	}
}

// TestEnvWaitEnvReadyOnFailedBootstrap checks that a failed bootstrap ends the
// wait with status "error" rather than "timeout": the condition can never hold,
// and the agent should stop waiting and read the message.
func TestEnvWaitEnvReadyOnFailedBootstrap(t *testing.T) {
	s, f := newTestServer(t)
	f.setExec(func(cmd []string) (int, string, error) {
		if len(cmd) > 0 && cmd[0] == "git" {
			return 128, "repository not found", nil
		}
		return 0, "", nil
	})
	env := mustCreate(t, s.registry, "app", "https://example.com/missing.git", "")

	_, out, err := s.envWait(context.Background(), nil, envWaitInput{EnvID: env.ID, Condition: condEnvReady})
	if err != nil {
		t.Fatalf("envWait: %v", err)
	}
	if out.Met {
		t.Error("met = true, want false for a failed bootstrap")
	}
	if out.Status != "error" {
		t.Errorf("status = %q, want error", out.Status)
	}
	if !strings.Contains(out.Detail, "repository not found") {
		t.Errorf("detail = %q, want the bootstrap failure", out.Detail)
	}
}

func TestEnvWaitEnvReadyTimesOutWhileCreating(t *testing.T) {
	s, f := newTestServer(t)

	release := make(chan struct{})
	defer close(release)
	f.setExec(func([]string) (int, string, error) {
		<-release
		return 0, "", nil
	})

	env, err := s.registry.Create(context.Background(), "slow", "", "go")
	if err != nil {
		t.Fatalf("Create: %v", err)
	}

	_, out, err := s.envWait(context.Background(), nil, envWaitInput{
		EnvID: env.ID, Condition: condEnvReady, TimeoutSec: 1,
	})
	if err != nil {
		t.Fatalf("envWait: %v", err)
	}
	if out.Met || out.Status != "timeout" {
		t.Errorf("out = %+v, want a timeout", out)
	}
	if out.WaitedSec < 1 {
		t.Errorf("waited %.2fs, want at least the 1s timeout", out.WaitedSec)
	}
}

func TestEnvWaitJobExited(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	stream, release := blockingStream("")
	f.setStream(stream)
	job, err := s.jobs.Start(env, "make", env.WorkDir)
	if err != nil {
		t.Fatalf("Start: %v", err)
	}

	go func() {
		time.Sleep(10 * time.Millisecond)
		close(release)
	}()

	_, out, err := s.envWait(context.Background(), nil, envWaitInput{
		EnvID: env.ID, Condition: condJobExited, JobID: job.ID,
	})
	if err != nil {
		t.Fatalf("envWait: %v", err)
	}
	if !out.Met || out.Status != "met" {
		t.Errorf("out = %+v, want met", out)
	}
	if out.JobID != job.ID {
		t.Errorf("job id = %q, want %q", out.JobID, job.ID)
	}
	if out.ExitCode == nil || *out.ExitCode != 0 {
		t.Errorf("exit code = %v, want 0", out.ExitCode)
	}
}

func TestEnvWaitJobExitedRejectsForeignJob(t *testing.T) {
	s, _ := newTestServer(t)
	envA := mustCreate(t, s.registry, "a", "", "go")
	envB := mustCreate(t, s.registry, "b", "", "go")

	job, _ := s.jobs.Start(envA, "true", envA.WorkDir)
	waitJobDone(t, job)

	_, _, err := s.envWait(context.Background(), nil, envWaitInput{
		EnvID: envB.ID, Condition: condJobExited, JobID: job.ID,
	})
	if err == nil || !strings.Contains(err.Error(), "belongs to") {
		t.Fatalf("err = %v, want a mismatched-environment error", err)
	}
}

func TestEnvWaitCommandSucceeds(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	var calls int32
	f.setExec(func([]string) (int, string, error) {
		if atomic.AddInt32(&calls, 1) < 3 {
			return 1, "connection refused", nil
		}
		return 0, "ok", nil
	})

	_, out, err := s.envWait(context.Background(), nil, envWaitInput{
		EnvID: env.ID, Condition: condCommandSucceeds, Command: "curl -sf localhost:8080",
	})
	if err != nil {
		t.Fatalf("envWait: %v", err)
	}
	if !out.Met || out.Status != "met" {
		t.Errorf("out = %+v, want met", out)
	}
	if atomic.LoadInt32(&calls) != 3 {
		t.Errorf("probed %d times, want 3", calls)
	}
}

func TestEnvWaitCommandSucceedsResolvesWorkDir(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	var gotWorkDir string
	f.setExec(func([]string) (int, string, error) { return 0, "", nil })

	if _, _, err := s.envWait(context.Background(), nil, envWaitInput{
		EnvID: env.ID, Condition: condCommandSucceeds, Command: "true", WorkDir: "sub",
	}); err != nil {
		t.Fatalf("envWait: %v", err)
	}
	for _, call := range f.execCalls() {
		if len(call.Cmd) == 3 && call.Cmd[2] == "true" {
			gotWorkDir = call.Opts.WorkDir
		}
	}
	if gotWorkDir != "/workspace/sub" {
		t.Errorf("work dir = %q, want /workspace/sub", gotWorkDir)
	}

	// The probe must obey the same workspace jail as the file tools.
	if _, _, err := s.envWait(context.Background(), nil, envWaitInput{
		EnvID: env.ID, Condition: condCommandSucceeds, Command: "true", WorkDir: "../etc",
	}); err == nil {
		t.Fatal("expected work_dir escaping the workspace to be rejected")
	}
}

func TestEnvWaitPortOpen(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	var calls int32
	f.setExec(func([]string) (int, string, error) {
		if atomic.AddInt32(&calls, 1) < 2 {
			return 0, procNetTCP(), nil
		}
		return 0, procNetTCP(listenEntry(8080)), nil
	})

	_, out, err := s.envWait(context.Background(), nil, envWaitInput{
		EnvID: env.ID, Condition: condPortOpen, Port: 8080,
	})
	if err != nil {
		t.Fatalf("envWait: %v", err)
	}
	if !out.Met || out.Status != "met" {
		t.Errorf("out = %+v, want met", out)
	}
}

func TestEnvWaitPortOpenTimesOut(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")
	f.setExec(func([]string) (int, string, error) { return 0, procNetTCP(), nil })

	_, out, err := s.envWait(context.Background(), nil, envWaitInput{
		EnvID: env.ID, Condition: condPortOpen, Port: 8080, TimeoutSec: 1,
	})
	if err != nil {
		t.Fatalf("a timeout must not be a tool error, got %v", err)
	}
	if out.Met || out.Status != "timeout" {
		t.Errorf("out = %+v, want a timeout", out)
	}
	if !strings.Contains(out.Detail, "nothing is listening") {
		t.Errorf("detail = %q, want the last observation", out.Detail)
	}
}

func TestEnvWaitFileConditions(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	t.Run("file_exists", func(t *testing.T) {
		f.setExec(func([]string) (int, string, error) { return 0, "", nil })
		_, out, err := s.envWait(context.Background(), nil, envWaitInput{
			EnvID: env.ID, Condition: condFileExists, Path: "target/app",
		})
		if err != nil || !out.Met {
			t.Fatalf("out = %+v, err = %v, want met", out, err)
		}
	})

	t.Run("file_matches", func(t *testing.T) {
		f.setExec(func([]string) (int, string, error) { return 0, "Listening on :8080\n", nil })
		_, out, err := s.envWait(context.Background(), nil, envWaitInput{
			EnvID: env.ID, Condition: condFileMatches, Path: "srv.log", Pattern: `Listening on :\d+`,
		})
		if err != nil || !out.Met {
			t.Fatalf("out = %+v, err = %v, want met", out, err)
		}
	})

	t.Run("path escaping the workspace is rejected", func(t *testing.T) {
		_, _, err := s.envWait(context.Background(), nil, envWaitInput{
			EnvID: env.ID, Condition: condFileExists, Path: "../../etc/shadow",
		})
		if err == nil || !strings.Contains(err.Error(), "must be under /workspace") {
			t.Fatalf("err = %v, want a jail violation", err)
		}
	})
}

func TestEnvWaitInputValidation(t *testing.T) {
	s, _ := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	tests := []struct {
		name    string
		in      envWaitInput
		wantErr string
	}{
		{"unknown environment", envWaitInput{EnvID: "env-99", Condition: condEnvReady}, "unknown environment"},
		{"unknown condition", envWaitInput{EnvID: env.ID, Condition: "vibes"}, `unknown condition "vibes"`},
		{"condition list in the error", envWaitInput{EnvID: env.ID, Condition: "vibes"}, condPortOpen},
		{"job_exited without a job", envWaitInput{EnvID: env.ID, Condition: condJobExited}, "job_id is required"},
		{"job_exited with an unknown job", envWaitInput{EnvID: env.ID, Condition: condJobExited, JobID: "job-9"}, "unknown job"},
		{"command_succeeds without a command", envWaitInput{EnvID: env.ID, Condition: condCommandSucceeds}, "requires command"},
		{"port_open without a port", envWaitInput{EnvID: env.ID, Condition: condPortOpen}, "between 1 and 65535"},
		{"port_open out of range", envWaitInput{EnvID: env.ID, Condition: condPortOpen, Port: 70000}, "between 1 and 65535"},
		{"file_exists without a path", envWaitInput{EnvID: env.ID, Condition: condFileExists}, "path is required"},
		{"file_matches without a pattern", envWaitInput{EnvID: env.ID, Condition: condFileMatches, Path: "a.log"}, "requires pattern"},
		{"file_matches with a bad pattern", envWaitInput{EnvID: env.ID, Condition: condFileMatches, Path: "a.log", Pattern: "([a-z"}, "pattern:"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, _, err := s.envWait(context.Background(), nil, tt.in)
			if err == nil || !strings.Contains(err.Error(), tt.wantErr) {
				t.Fatalf("err = %v, want it to contain %q", err, tt.wantErr)
			}
		})
	}
}

func TestEnvWaitClampsTimeout(t *testing.T) {
	s, _ := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	// An absurd timeout must not park the agent: it is clamped, and this
	// already-ready environment returns immediately either way.
	_, out, err := s.envWait(context.Background(), nil, envWaitInput{
		EnvID: env.ID, Condition: condEnvReady, TimeoutSec: 999999,
	})
	if err != nil || !out.Met {
		t.Fatalf("out = %+v, err = %v", out, err)
	}
}

// ---- background exec and job tools -----------------------------------------

func TestEnvExecBackgroundReturnsJobID(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	stream, release := blockingStream("booting\n")
	defer close(release)
	f.setStream(stream)

	res, out, err := s.envExec(context.Background(), nil, envExecInput{
		EnvID: env.ID, Command: "npm run dev", Background: true,
	})
	if err != nil {
		t.Fatalf("envExec: %v", err)
	}
	if out.JobID == "" {
		t.Fatal("background exec must return a job id")
	}
	if out.Status != string(StatusRunning) {
		t.Errorf("status = %q, want running", out.Status)
	}
	if out.ExitCode != nil {
		t.Error("a background job has no exit code yet")
	}
	if text := textOf(t, res); !strings.Contains(text, out.JobID) || !strings.Contains(text, "env_wait") {
		t.Errorf("result text = %q, want the job id and the follow-up tools", text)
	}
}

func TestEnvExecBackgroundRejectsUnknownEnv(t *testing.T) {
	s, _ := newTestServer(t)
	if _, _, err := s.envExec(context.Background(), nil, envExecInput{
		EnvID: "env-99", Command: "true", Background: true,
	}); err == nil {
		t.Fatal("expected an unknown environment to be rejected")
	}
}

func TestEnvJobListAndOutput(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")
	f.setExec(func([]string) (int, string, error) { return 0, "one\ntwo\nthree\n", nil })

	_, exec, err := s.envExec(context.Background(), nil, envExecInput{
		EnvID: env.ID, Command: "make", Background: true,
	})
	if err != nil {
		t.Fatalf("envExec: %v", err)
	}
	job, _ := s.jobs.Get(exec.JobID)
	waitJobDone(t, job)

	_, list, err := s.envJobList(context.Background(), nil, envJobListInput{})
	if err != nil {
		t.Fatalf("envJobList: %v", err)
	}
	if len(list.Jobs) != 1 || list.Jobs[0].JobID != exec.JobID {
		t.Fatalf("jobs = %+v, want just %s", list.Jobs, exec.JobID)
	}
	if list.Jobs[0].Command != "make" || list.Jobs[0].Status != string(StatusExited) {
		t.Errorf("job = %+v", list.Jobs[0])
	}

	_, out, err := s.envJobOutput(context.Background(), nil, envJobOutputInput{JobID: exec.JobID, Tail: 2})
	if err != nil {
		t.Fatalf("envJobOutput: %v", err)
	}
	if out.Output != "two\nthree" {
		t.Errorf("output = %q, want the last two lines", out.Output)
	}
	if out.Job.ExitCode == nil || *out.Job.ExitCode != 0 {
		t.Errorf("exit code = %v, want 0", out.Job.ExitCode)
	}
}

func TestEnvJobListFiltersByEnvironment(t *testing.T) {
	s, _ := newTestServer(t)
	envA := mustCreate(t, s.registry, "a", "", "go")
	envB := mustCreate(t, s.registry, "b", "", "go")

	jobA, _ := s.jobs.Start(envA, "true", envA.WorkDir)
	jobB, _ := s.jobs.Start(envB, "true", envB.WorkDir)
	waitJobDone(t, jobA)
	waitJobDone(t, jobB)

	_, list, err := s.envJobList(context.Background(), nil, envJobListInput{EnvID: envB.ID})
	if err != nil {
		t.Fatalf("envJobList: %v", err)
	}
	if len(list.Jobs) != 1 || list.Jobs[0].JobID != jobB.ID {
		t.Errorf("jobs = %+v, want only %s", list.Jobs, jobB.ID)
	}
}

func TestEnvJobListEmpty(t *testing.T) {
	s, _ := newTestServer(t)
	res, list, err := s.envJobList(context.Background(), nil, envJobListInput{})
	if err != nil {
		t.Fatalf("envJobList: %v", err)
	}
	if len(list.Jobs) != 0 {
		t.Errorf("jobs = %+v, want none", list.Jobs)
	}
	if got := textOf(t, res); got != "no jobs" {
		t.Errorf("text = %q, want %q", got, "no jobs")
	}
}

func TestEnvJobOutputUnknownJob(t *testing.T) {
	s, _ := newTestServer(t)
	if _, _, err := s.envJobOutput(context.Background(), nil, envJobOutputInput{JobID: "job-9"}); err == nil {
		t.Fatal("expected an error for an unknown job")
	}
}

func TestEnvJobKillHandler(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	stream, release := blockingStream("")
	defer close(release)
	f.setStream(stream)

	_, exec, err := s.envExec(context.Background(), nil, envExecInput{
		EnvID: env.ID, Command: "serve", Background: true,
	})
	if err != nil {
		t.Fatalf("envExec: %v", err)
	}

	_, out, err := s.envJobKill(context.Background(), nil, envJobKillInput{JobID: exec.JobID, Force: true})
	if err != nil {
		t.Fatalf("envJobKill: %v", err)
	}
	if out.Status != string(StatusKilled) {
		t.Errorf("status = %q, want killed", out.Status)
	}
}

func TestEnvDestroyKillsJobs(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	stream, release := blockingStream("")
	defer close(release)
	f.setStream(stream)

	job, err := s.jobs.Start(env, "serve", env.WorkDir)
	if err != nil {
		t.Fatalf("Start: %v", err)
	}

	if _, _, err := s.envDestroy(context.Background(), nil, envIDInput{EnvID: env.ID}); err != nil {
		t.Fatalf("envDestroy: %v", err)
	}
	if job.Status() == StatusRunning {
		t.Error("destroying an environment must stop its jobs")
	}
	if _, ok := s.jobs.Get(job.ID); ok {
		t.Error("destroying an environment must forget its jobs")
	}
}

// ---- rendering --------------------------------------------------------------

func TestRenderWait(t *testing.T) {
	t.Run("met with detail", func(t *testing.T) {
		got := renderWait(envWaitOutput{
			EnvID: "env-1", Condition: condPortOpen, Met: true, Status: "met",
			WaitedSec: 31.44, Detail: "a process is listening on port 8080",
		})
		want := "env=env-1 condition=port_open status=met met=true waited=31.4s\na process is listening on port 8080"
		if got != want {
			t.Errorf("got:\n%s\nwant:\n%s", got, want)
		}
	})

	t.Run("job exit is reported", func(t *testing.T) {
		code := 2
		got := renderWait(envWaitOutput{
			EnvID: "env-1", Condition: condJobExited, Met: true, Status: "met",
			JobID: "job-3", ExitCode: &code,
		})
		if !strings.Contains(got, "job=job-3") || !strings.Contains(got, "exit=2") {
			t.Errorf("got %q", got)
		}
	})

	t.Run("timeout without detail", func(t *testing.T) {
		got := renderWait(envWaitOutput{EnvID: "env-1", Condition: condEnvReady, Status: "timeout"})
		want := "env=env-1 condition=env_ready status=timeout met=false waited=0.0s"
		if got != want {
			t.Errorf("got %q, want %q", got, want)
		}
	})
}

func TestRenderJobInfo(t *testing.T) {
	code := 0
	got := renderJobInfo(jobInfoOutput{
		JobID: "job-1", EnvID: "env-1", Command: "npm run dev", Status: "exited",
		ExitCode: &code, ElapsedSec: 12.34,
	})
	want := `job-1 env=env-1 status=exited exit=0 elapsed=12.3s command="npm run dev"`
	if got != want {
		t.Errorf("got %q, want %q", got, want)
	}

	running := renderJobInfo(jobInfoOutput{JobID: "job-2", EnvID: "env-1", Command: "serve", Status: "running"})
	if strings.Contains(running, "exit=") {
		t.Errorf("a running job must not render an exit code: %q", running)
	}

	failed := renderJobInfo(jobInfoOutput{JobID: "job-3", EnvID: "env-1", Command: "serve", Status: "failed", Error: "boom"})
	if !strings.Contains(failed, `error="boom"`) {
		t.Errorf("got %q", failed)
	}
}

// TestEnvWaitOverTransport exercises the tool end to end, so the schema and
// dispatch for the new condition parameters are covered too.
func TestEnvWaitOverTransport(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")
	f.setExec(func([]string) (int, string, error) { return 0, procNetTCP(listenEntry(3000)), nil })

	cs := connect(t, s)
	res, err := cs.CallTool(context.Background(), &mcp.CallToolParams{
		Name:      "env_wait",
		Arguments: map[string]any{"env_id": env.ID, "condition": "port_open", "port": 3000},
	})
	if err != nil {
		t.Fatalf("CallTool: %v", err)
	}
	if res.IsError {
		t.Fatalf("env_wait reported an error: %+v", res.Content)
	}
	if text := textOf(t, res); !strings.Contains(text, "status=met") {
		t.Errorf("text = %q", text)
	}
}
