package main

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"time"
)

// JobStatus tracks the lifecycle state of a background command.
type JobStatus string

const (
	// StatusRunning means the command is still executing.
	StatusRunning JobStatus = "running"
	// StatusExited means the command finished on its own; ExitCode is set.
	StatusExited JobStatus = "exited"
	// StatusKilled means env_job_kill (or env_destroy) stopped the command.
	StatusKilled JobStatus = "killed"
	// StatusFailed means the command could not be run at all.
	StatusFailed JobStatus = "failed"
)

// jobDir holds one pid file per background job inside the container. Jobs run
// under `sh -c 'echo $$ >pidfile; exec cmd'`, so the recorded pid belongs to
// the command itself rather than to a wrapper shell.
const jobDir = "/tmp/devenv-mcp-jobs"

// defaultKillGrace is how long a killed job has to handle SIGTERM before it
// is sent SIGKILL.
const defaultKillGrace = 5 * time.Second

// defaultMaxJobs caps concurrently running background jobs across all
// environments. Each one holds a podman exec client and an output buffer.
const defaultMaxJobs = 50

// Job is one background command running inside an environment. Fields above
// the mutex are set once at creation; everything below it is written by the
// goroutine running the command while tool handlers read it.
type Job struct {
	ID          string
	EnvID       string
	ContainerID ContainerID
	Command     string
	WorkDir     string
	StartedAt   time.Time

	out     *outputBuffer
	cancel  context.CancelFunc
	doneCh  chan struct{} // closed when the command goroutine returns
	pidFile string

	mu       sync.Mutex
	status   JobStatus
	exitCode *int
	errMsg   string
	endedAt  time.Time
	killed   bool // a kill was requested, so a non-zero exit is not a failure
}

// Status returns the current job status.
func (j *Job) Status() JobStatus {
	j.mu.Lock()
	defer j.mu.Unlock()
	return j.status
}

// ExitCode returns the command's exit code, or nil while it is still running.
func (j *Job) ExitCode() *int {
	j.mu.Lock()
	defer j.mu.Unlock()
	return j.exitCode
}

// Error returns the failure message for a job that could not be run.
func (j *Job) Error() string {
	j.mu.Lock()
	defer j.mu.Unlock()
	return j.errMsg
}

// Done returns a channel closed once the job is no longer running.
func (j *Job) Done() <-chan struct{} { return j.doneCh }

// ElapsedSec reports the job's runtime, frozen once it finishes.
func (j *Job) ElapsedSec() float64 {
	j.mu.Lock()
	defer j.mu.Unlock()
	if j.endedAt.IsZero() {
		return time.Since(j.StartedAt).Seconds()
	}
	return j.endedAt.Sub(j.StartedAt).Seconds()
}

// Output returns the captured output so far. It is safe to call while the job
// is still running.
func (j *Job) Output() string { return renderOutput(j.out) }

func (j *Job) markKilling() {
	j.mu.Lock()
	defer j.mu.Unlock()
	j.killed = true
}

// finish records the terminal state of a job exactly once.
func (j *Job) finish(code int, err error) {
	j.mu.Lock()
	defer j.mu.Unlock()
	j.endedAt = time.Now()
	switch {
	case err != nil:
		j.status = StatusFailed
		j.errMsg = err.Error()
	case j.killed:
		j.status = StatusKilled
		j.exitCode = &code
	default:
		j.status = StatusExited
		j.exitCode = &code
	}
}

// JobRegistry owns every background command across all environments. Jobs live
// only in memory: a server restart leaves the container running but abandons
// the job, so Reconcile does not recover them.
type JobRegistry struct {
	mu       sync.Mutex
	counter  int
	jobs     map[string]*Job
	provider ContainerProvider
	maxJobs  int

	// killGrace is the SIGTERM-to-SIGKILL delay, overridden in tests.
	killGrace time.Duration
}

// NewJobRegistry creates a job registry backed by the given provider.
func NewJobRegistry(provider ContainerProvider, maxJobs int) *JobRegistry {
	return &JobRegistry{
		jobs:      map[string]*Job{},
		provider:  provider,
		maxJobs:   maxJobs,
		killGrace: defaultKillGrace,
	}
}

// Start launches command in the background and returns immediately. The job
// keeps running until it exits on its own, is killed, or its environment is
// destroyed.
func (r *JobRegistry) Start(env *Environment, command, workDir string) (*Job, error) {
	r.mu.Lock()
	if r.maxJobs > 0 && r.countRunningLocked() >= r.maxJobs {
		r.mu.Unlock()
		return nil, fmt.Errorf("maximum %d running jobs reached: kill one with env_job_kill", r.maxJobs)
	}
	r.counter++
	id := fmt.Sprintf("job-%d", r.counter)
	r.mu.Unlock()

	ctx, cancel := context.WithCancel(context.Background())
	job := &Job{
		ID:          id,
		EnvID:       env.ID,
		ContainerID: env.ContainerID,
		Command:     command,
		WorkDir:     workDir,
		StartedAt:   time.Now(),
		out:         newOutputBuffer(maxExecOutput),
		cancel:      cancel,
		doneCh:      make(chan struct{}),
		pidFile:     jobDir + "/" + id + ".pid",
		status:      StatusRunning,
	}

	r.mu.Lock()
	r.jobs[id] = job
	r.mu.Unlock()

	// The pid file lets Kill signal the process inside the container. Killing
	// the local podman client alone is not enough: the process it started keeps
	// running in the container.
	wrapped := fmt.Sprintf("mkdir -p %s; echo $$ > %s; exec %s",
		shellQuote(jobDir), shellQuote(job.pidFile), command)

	go func() {
		defer close(job.doneCh)
		defer cancel()
		code, err := r.provider.ExecStream(ctx, job.ContainerID,
			[]string{"sh", "-c", wrapped}, ExecOpts{WorkDir: workDir}, job.out)
		job.finish(code, err)
	}()

	return job, nil
}

// countRunningLocked counts jobs that still hold a container process. The
// caller must hold r.mu.
func (r *JobRegistry) countRunningLocked() int {
	n := 0
	for _, j := range r.jobs {
		if j.Status() == StatusRunning {
			n++
		}
	}
	return n
}

// Get returns a job by ID.
func (r *JobRegistry) Get(id string) (*Job, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	j, ok := r.jobs[id]
	return j, ok
}

// List returns jobs for one environment, or all jobs when envID is empty.
func (r *JobRegistry) List(envID string) []*Job {
	r.mu.Lock()
	defer r.mu.Unlock()
	jobs := make([]*Job, 0, len(r.jobs))
	for _, j := range r.jobs {
		if envID == "" || j.EnvID == envID {
			jobs = append(jobs, j)
		}
	}
	return jobs
}

// Kill stops a running job: SIGTERM inside the container, then SIGKILL after
// the grace period if it has not exited. Killing an already-finished job is a
// no-op. Only the command itself is signalled — processes it spawned in turn
// may outlive it.
func (r *JobRegistry) Kill(ctx context.Context, id string, force bool) (*Job, error) {
	job, ok := r.Get(id)
	if !ok {
		return nil, fmt.Errorf("unknown job %q", id)
	}
	if job.Status() != StatusRunning {
		return job, nil
	}

	job.markKilling()

	signal := "TERM"
	if force {
		signal = "KILL"
	}
	r.signal(ctx, job, signal)

	if force {
		job.cancel()
		<-job.doneCh
		return job, nil
	}

	select {
	case <-job.doneCh:
		return job, nil
	case <-time.After(r.killGrace):
	}

	// Still alive after the grace period: escalate, then drop the podman client
	// so the goroutine cannot hang on an unresponsive exec.
	r.signal(ctx, job, "KILL")
	job.cancel()
	<-job.doneCh
	return job, nil
}

// signal sends one signal to the job's recorded pid, ignoring failures: the
// pid file may not exist yet, and the process may have exited already.
func (r *JobRegistry) signal(ctx context.Context, job *Job, sig string) {
	cmd := fmt.Sprintf("kill -%s \"$(cat %s)\" 2>/dev/null || true", sig, shellQuote(job.pidFile))
	_, _, _ = r.provider.Exec(ctx, job.ContainerID, []string{"sh", "-c", cmd}, ExecOpts{})
}

// KillEnv stops and forgets every job belonging to an environment. It is called
// when the environment is destroyed, where the container is about to disappear,
// so signalling is best-effort and cancellation does the real work.
func (r *JobRegistry) KillEnv(ctx context.Context, envID string) {
	for _, job := range r.List(envID) {
		if job.Status() == StatusRunning {
			job.markKilling()
			r.signal(ctx, job, "KILL")
			job.cancel()
			<-job.doneCh
		}
		r.mu.Lock()
		delete(r.jobs, job.ID)
		r.mu.Unlock()
	}
}

// tailLines returns the last n lines of s, or all of it when n <= 0.
func tailLines(s string, n int) string {
	if n <= 0 || s == "" {
		return s
	}
	lines := strings.Split(strings.TrimSuffix(s, "\n"), "\n")
	if len(lines) <= n {
		return s
	}
	return strings.Join(lines[len(lines)-n:], "\n")
}
