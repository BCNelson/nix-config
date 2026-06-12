package main

import (
	"context"
	"fmt"
	"sync"
	"time"
)

type jobStatus string

const (
	statusRunning  jobStatus = "running"
	statusExited   jobStatus = "exited"
	statusCanceled jobStatus = "canceled"
	statusError    jobStatus = "error"
)

// job is a background command execution tracked by the registry so agents can
// start a long-running command and later wait on, poll, or cancel it across
// multiple tool calls.
type job struct {
	id      string
	host    string
	command string
	out     *outputBuffer
	start   time.Time
	cancel  context.CancelFunc
	done    chan struct{}

	mu       sync.Mutex
	status   jobStatus
	exitCode int
	errMsg   string
}

// jobSnapshot is an immutable view of a job's current state.
type jobSnapshot struct {
	JobID      string
	Host       string
	Command    string
	Status     jobStatus
	ExitCode   *int
	Output     string
	Truncated  bool
	TotalBytes int64
	ElapsedSec float64
	Error      string
}

func (j *job) snapshot() jobSnapshot {
	j.mu.Lock()
	status := j.status
	code := j.exitCode
	errMsg := j.errMsg
	j.mu.Unlock()

	output, truncated, total := j.out.snapshot()
	snap := jobSnapshot{
		JobID:      j.id,
		Host:       j.host,
		Command:    j.command,
		Status:     status,
		Output:     output,
		Truncated:  truncated,
		TotalBytes: total,
		ElapsedSec: time.Since(j.start).Seconds(),
		Error:      errMsg,
	}
	if status == statusExited {
		c := code
		snap.ExitCode = &c
	}
	return snap
}

// jobRegistry holds all background jobs for the process lifetime.
type jobRegistry struct {
	mu      sync.Mutex
	counter int
	jobs    map[string]*job
}

func newJobRegistry() *jobRegistry {
	return &jobRegistry{jobs: map[string]*job{}}
}

// start launches command on tgt in the background and returns immediately.
// parent should be a long-lived context (e.g. context.Background()) so the job
// outlives the tool call that created it; cancellation is driven by the job's
// own cancel func (see ssh_cancel).
func (r *jobRegistry) start(parent context.Context, tgt target, command string) *job {
	r.mu.Lock()
	r.counter++
	id := fmt.Sprintf("job-%d", r.counter)
	r.mu.Unlock()

	ctx, cancel := context.WithCancel(parent)
	j := &job{
		id:      id,
		host:    tgt.name,
		command: command,
		out:     newOutputBuffer(maxOutput),
		start:   time.Now(),
		cancel:  cancel,
		done:    make(chan struct{}),
		status:  statusRunning,
	}

	r.mu.Lock()
	r.jobs[id] = j
	r.mu.Unlock()

	go func() {
		code, err := runCommand(ctx, tgt, command, j.out)
		j.mu.Lock()
		switch {
		case err != nil && ctx.Err() != nil:
			j.status = statusCanceled
		case err != nil:
			j.status = statusError
			j.errMsg = err.Error()
		default:
			j.status = statusExited
			j.exitCode = code
		}
		j.mu.Unlock()
		close(j.done)
	}()

	return j
}

func (r *jobRegistry) get(id string) (*job, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	j, ok := r.jobs[id]
	return j, ok
}

func (r *jobRegistry) list() []jobSnapshot {
	r.mu.Lock()
	jobs := make([]*job, 0, len(r.jobs))
	for _, j := range r.jobs {
		jobs = append(jobs, j)
	}
	r.mu.Unlock()

	snaps := make([]jobSnapshot, 0, len(jobs))
	for _, j := range jobs {
		snaps = append(snaps, j.snapshot())
	}
	return snaps
}
