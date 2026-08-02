package main

import (
	"context"
	"errors"
	"io"
	"strings"
	"testing"
	"time"
)

// blockingStream returns a stream hook that writes banner, then blocks until
// either the returned release channel is closed or the context is cancelled.
// It models a long-running command such as a dev server.
func blockingStream(banner string) (func(context.Context, []string, io.Writer) (int, error), chan struct{}) {
	release := make(chan struct{})
	fn := func(ctx context.Context, _ []string, out io.Writer) (int, error) {
		if banner != "" {
			_, _ = out.Write([]byte(banner))
		}
		select {
		case <-release:
			return 0, nil
		case <-ctx.Done():
			return 137, nil // as if SIGKILLed
		}
	}
	return fn, release
}

// waitJobDone blocks until the job finishes, failing the test if it hangs.
func waitJobDone(t *testing.T, job *Job) {
	t.Helper()
	select {
	case <-job.Done():
	case <-time.After(5 * time.Second):
		t.Fatalf("job %s did not finish within 5s", job.ID)
	}
}

func TestJobStartCapturesOutputWhileRunning(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	stream, release := blockingStream("listening on 8080\n")
	f.setStream(stream)

	job, err := s.jobs.Start(env, "serve", env.WorkDir)
	if err != nil {
		t.Fatalf("Start: %v", err)
	}
	if job.Status() != StatusRunning {
		t.Errorf("status = %q, want running", job.Status())
	}
	if job.ExitCode() != nil {
		t.Errorf("a running job must not report an exit code, got %d", *job.ExitCode())
	}

	// Output is readable before the command finishes.
	deadline := time.Now().Add(2 * time.Second)
	for job.Output() == "" && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	if !strings.Contains(job.Output(), "listening on 8080") {
		t.Errorf("output = %q, want the banner while still running", job.Output())
	}

	close(release)
	waitJobDone(t, job)

	if job.Status() != StatusExited {
		t.Errorf("status = %q, want exited", job.Status())
	}
	if code := job.ExitCode(); code == nil || *code != 0 {
		t.Errorf("exit code = %v, want 0", code)
	}
	if job.ElapsedSec() <= 0 {
		t.Error("elapsed should be positive once the job has run")
	}
}

// TestJobStartWrapsCommandWithPidFile checks the wrapper that makes an
// in-container kill possible: the pid recorded must be the command's own,
// which is what `exec` guarantees.
func TestJobStartWrapsCommandWithPidFile(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	job, err := s.jobs.Start(env, "cargo run", "/workspace/sub")
	if err != nil {
		t.Fatalf("Start: %v", err)
	}
	waitJobDone(t, job)

	var wrapped string
	for _, call := range f.execCalls() {
		if len(call.Cmd) == 3 && strings.Contains(call.Cmd[2], "cargo run") {
			wrapped = call.Cmd[2]
			if call.Opts.WorkDir != "/workspace/sub" {
				t.Errorf("work dir = %q, want /workspace/sub", call.Opts.WorkDir)
			}
		}
	}
	if wrapped == "" {
		t.Fatalf("the command was never executed: %+v", f.execCalls())
	}
	for _, want := range []string{"echo $$ >", jobDir + "/" + job.ID + ".pid", "exec cargo run"} {
		if !strings.Contains(wrapped, want) {
			t.Errorf("wrapper %q is missing %q", wrapped, want)
		}
	}
}

func TestJobRecordsNonZeroExit(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")
	f.setExec(func(_ []string) (int, string, error) { return 2, "build failed\n", nil })

	job, err := s.jobs.Start(env, "make", env.WorkDir)
	if err != nil {
		t.Fatalf("Start: %v", err)
	}
	waitJobDone(t, job)

	if job.Status() != StatusExited {
		t.Errorf("status = %q, want exited", job.Status())
	}
	if code := job.ExitCode(); code == nil || *code != 2 {
		t.Errorf("exit code = %v, want 2", code)
	}
	if !strings.Contains(job.Output(), "build failed") {
		t.Errorf("output = %q", job.Output())
	}
}

func TestJobRecordsProviderFailure(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")
	f.setStream(func(context.Context, []string, io.Writer) (int, error) {
		return -1, errors.New("podman is not running")
	})

	job, err := s.jobs.Start(env, "serve", env.WorkDir)
	if err != nil {
		t.Fatalf("Start: %v", err)
	}
	waitJobDone(t, job)

	if job.Status() != StatusFailed {
		t.Errorf("status = %q, want failed", job.Status())
	}
	if !strings.Contains(job.Error(), "podman is not running") {
		t.Errorf("error = %q", job.Error())
	}
	if job.ExitCode() != nil {
		t.Error("a job that never ran must not report an exit code")
	}
}

// TestJobKillSignalsInsideContainer checks the SIGTERM path: the signal is
// delivered to the recorded pid, and the job ends as killed rather than as an
// ordinary non-zero exit.
func TestJobKillSignalsInsideContainer(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	stream, release := blockingStream("")
	f.setStream(stream)
	// The command exits when it "receives" the signal the kill sends.
	f.setExec(func(cmd []string) (int, string, error) {
		if len(cmd) == 3 && strings.Contains(cmd[2], "kill -TERM") {
			close(release)
		}
		return 0, "", nil
	})

	job, err := s.jobs.Start(env, "serve", env.WorkDir)
	if err != nil {
		t.Fatalf("Start: %v", err)
	}

	killed, err := s.jobs.Kill(context.Background(), job.ID, false)
	if err != nil {
		t.Fatalf("Kill: %v", err)
	}
	if killed.Status() != StatusKilled {
		t.Errorf("status = %q, want killed", killed.Status())
	}

	var signalled string
	for _, call := range f.execCalls() {
		if len(call.Cmd) == 3 && strings.Contains(call.Cmd[2], "kill -TERM") {
			signalled = call.Cmd[2]
		}
	}
	if !strings.Contains(signalled, job.pidFile) {
		t.Errorf("SIGTERM command %q does not read the job's pid file %q", signalled, job.pidFile)
	}
}

// TestJobKillEscalatesToSIGKILL covers a command that ignores SIGTERM: after
// the grace period it must be SIGKILLed and the wait must still return.
func TestJobKillEscalatesToSIGKILL(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	stream, release := blockingStream("")
	defer close(release)
	f.setStream(stream) // ignores every signal; only ctx cancellation ends it

	job, err := s.jobs.Start(env, "stubborn", env.WorkDir)
	if err != nil {
		t.Fatalf("Start: %v", err)
	}

	start := time.Now()
	killed, err := s.jobs.Kill(context.Background(), job.ID, false)
	if err != nil {
		t.Fatalf("Kill: %v", err)
	}
	if elapsed := time.Since(start); elapsed < s.jobs.killGrace {
		t.Errorf("kill returned after %s, want at least the %s grace period", elapsed, s.jobs.killGrace)
	}
	if killed.Status() != StatusKilled {
		t.Errorf("status = %q, want killed", killed.Status())
	}

	var sawKill bool
	for _, call := range f.execCalls() {
		if len(call.Cmd) == 3 && strings.Contains(call.Cmd[2], "kill -KILL") {
			sawKill = true
		}
	}
	if !sawKill {
		t.Error("expected an escalation to SIGKILL after the grace period")
	}
}

func TestJobKillForceSkipsGrace(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	stream, release := blockingStream("")
	defer close(release)
	f.setStream(stream)

	job, err := s.jobs.Start(env, "serve", env.WorkDir)
	if err != nil {
		t.Fatalf("Start: %v", err)
	}

	start := time.Now()
	if _, err := s.jobs.Kill(context.Background(), job.ID, true); err != nil {
		t.Fatalf("Kill: %v", err)
	}
	if elapsed := time.Since(start); elapsed >= s.jobs.killGrace {
		t.Errorf("force kill took %s, it must not wait out the grace period", elapsed)
	}

	for _, call := range f.execCalls() {
		if len(call.Cmd) == 3 && strings.Contains(call.Cmd[2], "kill -TERM") {
			t.Error("force kill must not send SIGTERM first")
		}
	}
}

func TestJobKillFinishedJobIsNoOp(t *testing.T) {
	s, _ := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")

	job, err := s.jobs.Start(env, "true", env.WorkDir)
	if err != nil {
		t.Fatalf("Start: %v", err)
	}
	waitJobDone(t, job)

	killed, err := s.jobs.Kill(context.Background(), job.ID, false)
	if err != nil {
		t.Fatalf("killing a finished job should not error: %v", err)
	}
	if killed.Status() != StatusExited {
		t.Errorf("status = %q, want the original exited status preserved", killed.Status())
	}
}

func TestJobKillUnknownJob(t *testing.T) {
	s, _ := newTestServer(t)
	if _, err := s.jobs.Kill(context.Background(), "job-99", false); err == nil {
		t.Fatal("expected an error for an unknown job")
	}
}

func TestJobRegistryEnforcesMaxRunning(t *testing.T) {
	s, f := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")
	s.jobs.maxJobs = 2

	stream, release := blockingStream("")
	defer close(release)
	f.setStream(stream)

	for i := 0; i < 2; i++ {
		if _, err := s.jobs.Start(env, "serve", env.WorkDir); err != nil {
			t.Fatalf("Start %d: %v", i, err)
		}
	}
	if _, err := s.jobs.Start(env, "serve", env.WorkDir); err == nil {
		t.Fatal("expected the third concurrent job to be rejected")
	} else if !strings.Contains(err.Error(), "maximum 2") {
		t.Errorf("error = %v, want it to mention the cap", err)
	}
}

// TestJobRegistryCapCountsOnlyRunningJobs checks that finished jobs free their
// slot, so a long session is not blocked by history.
func TestJobRegistryCapCountsOnlyRunningJobs(t *testing.T) {
	s, _ := newTestServer(t)
	env := mustCreate(t, s.registry, "app", "", "go")
	s.jobs.maxJobs = 1

	first, err := s.jobs.Start(env, "true", env.WorkDir)
	if err != nil {
		t.Fatalf("Start: %v", err)
	}
	waitJobDone(t, first)

	if _, err := s.jobs.Start(env, "true", env.WorkDir); err != nil {
		t.Fatalf("a finished job must not hold its slot: %v", err)
	}
}

func TestJobRegistryListFiltersByEnv(t *testing.T) {
	s, _ := newTestServer(t)
	envA := mustCreate(t, s.registry, "a", "", "go")
	envB := mustCreate(t, s.registry, "b", "", "go")

	jobA, _ := s.jobs.Start(envA, "true", envA.WorkDir)
	jobB, _ := s.jobs.Start(envB, "true", envB.WorkDir)
	waitJobDone(t, jobA)
	waitJobDone(t, jobB)

	if got := s.jobs.List(envA.ID); len(got) != 1 || got[0].ID != jobA.ID {
		t.Errorf("List(%q) = %+v, want only %s", envA.ID, got, jobA.ID)
	}
	if got := s.jobs.List(""); len(got) != 2 {
		t.Errorf("List(\"\") returned %d jobs, want 2", len(got))
	}
}

func TestJobRegistryKillEnvStopsAndForgets(t *testing.T) {
	s, f := newTestServer(t)
	envA := mustCreate(t, s.registry, "a", "", "go")
	envB := mustCreate(t, s.registry, "b", "", "go")

	stream, release := blockingStream("")
	defer close(release)
	f.setStream(stream)

	jobA, _ := s.jobs.Start(envA, "serve", envA.WorkDir)
	jobB, _ := s.jobs.Start(envB, "serve", envB.WorkDir)

	s.jobs.KillEnv(context.Background(), envA.ID)

	if _, ok := s.jobs.Get(jobA.ID); ok {
		t.Error("a destroyed environment's jobs must be forgotten")
	}
	if jobA.Status() != StatusKilled {
		t.Errorf("status = %q, want killed", jobA.Status())
	}
	if _, ok := s.jobs.Get(jobB.ID); !ok {
		t.Error("jobs in other environments must be left alone")
	}
	if jobB.Status() != StatusRunning {
		t.Errorf("job B status = %q, want it still running", jobB.Status())
	}
}

func TestTailLines(t *testing.T) {
	tests := []struct {
		name string
		in   string
		n    int
		want string
	}{
		{"zero returns everything", "a\nb\nc\n", 0, "a\nb\nc\n"},
		{"negative returns everything", "a\nb\nc\n", -1, "a\nb\nc\n"},
		{"empty input", "", 5, ""},
		{"fewer lines than requested", "a\nb\n", 5, "a\nb\n"},
		{"exact", "a\nb\n", 2, "a\nb\n"},
		{"tail", "a\nb\nc\nd\n", 2, "c\nd"},
		{"no trailing newline", "a\nb\nc", 2, "b\nc"},
		{"single line", "only", 1, "only"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := tailLines(tt.in, tt.n); got != tt.want {
				t.Errorf("tailLines(%q, %d) = %q, want %q", tt.in, tt.n, got, tt.want)
			}
		})
	}
}
