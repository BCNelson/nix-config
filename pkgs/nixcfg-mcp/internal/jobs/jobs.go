// Package jobs runs long builds as durable background jobs. Each job is a
// transient systemd user unit in lowprio.slice writing to files under
// $XDG_STATE_HOME/nixcfg/jobs/<id>/, so it survives the MCP server or CLI that
// started it, and anyone can follow it:
//
//	job.json  what was run
//	log       combined output
//	exit      the exit status, written when it finishes
package jobs

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"time"

	"mvdan.cc/sh/v3/syntax"
	"nixcfg-mcp/internal/config"
)

type Status string

const (
	Running   Status = "running"
	Succeeded Status = "succeeded"
	Failed    Status = "failed"
	Lost      Status = "lost" // the process vanished without recording an exit
	Cancelled Status = "cancelled"
)

// Job describes one background job.
type Job struct {
	ID      string            `json:"job_id"`
	Kind    string            `json:"kind"` // check-host | build | preseed | ...
	Host    string            `json:"host,omitempty"`
	Argv    []string          `json:"argv"`
	Dir     string            `json:"dir"`
	Unit    string            `json:"unit,omitempty"`
	PID     int               `json:"pid,omitempty"` // fallback runner without systemd
	Started time.Time         `json:"started"`
	Meta    map[string]string `json:"meta,omitempty"`

	// Filled in by Status.
	Status   Status    `json:"status"`
	Exit     *int      `json:"exit_code,omitempty"`
	Finished time.Time `json:"finished,omitempty"`
	Elapsed  string    `json:"elapsed"`
	Tail     string    `json:"tail,omitempty"`
	Error    string    `json:"error,omitempty"` // the nix error block, when it failed
}

func root() string { return config.StateDir("jobs") }

func dir(id string) string { return filepath.Join(root(), id) }

func validID(id string) bool {
	return strings.HasPrefix(id, "j-") && !strings.ContainsAny(id, "/\\.")
}

// Start launches argv in cwd as a background job.
func Start(kind, host string, argv []string, cwd string, meta map[string]string) (*Job, error) {
	b := make([]byte, 3)
	_, _ = rand.Read(b)
	now := time.Now()
	id := "j-" + now.UTC().Format("20060102-150405") + "-" + hex.EncodeToString(b)
	d := dir(id)
	if err := os.MkdirAll(d, 0o700); err != nil {
		return nil, err
	}
	j := &Job{ID: id, Kind: kind, Host: host, Argv: argv, Dir: cwd, Started: now, Meta: meta, Status: Running}

	var quoted []string
	for _, a := range argv {
		q, err := syntax.Quote(a, syntax.LangBash)
		if err != nil {
			return nil, fmt.Errorf("can't quote %q: %v", a, err)
		}
		quoted = append(quoted, q)
	}
	logf, exitf := filepath.Join(d, "log"), filepath.Join(d, "exit")
	script := fmt.Sprintf("cd %s || exit 97; %s >%s 2>&1 </dev/null; echo $? >%s.tmp && mv %s.tmp %s",
		q(cwd), strings.Join(quoted, " "), q(logf), q(exitf), q(exitf), q(exitf))

	j.Unit = "nixcfg-" + id
	cmd := exec.Command("systemd-run", "--user", "--quiet", "--collect", "--unit="+j.Unit, "--slice=lowprio.slice",
		"--property=KillMode=control-group", "--setenv=PATH="+os.Getenv("PATH"), "--setenv=HOME="+os.Getenv("HOME"),
		"--", "bash", "-c", script)
	if out, err := cmd.CombinedOutput(); err != nil {
		// No user manager (e.g. inside a container): detach a plain process.
		j.Unit = ""
		c := exec.Command("nice", "-n", "15", "bash", "-c", script)
		c.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
		if err2 := c.Start(); err2 != nil {
			return nil, fmt.Errorf("systemd-run: %v: %s; fallback: %v", err, strings.TrimSpace(string(out)), err2)
		}
		j.PID = c.Process.Pid
		_ = c.Process.Release()
	}
	raw, _ := json.MarshalIndent(j, "", "  ")
	if err := os.WriteFile(filepath.Join(d, "job.json"), raw, 0o600); err != nil {
		return nil, err
	}
	return Get(id)
}

func q(s string) string {
	out, _ := syntax.Quote(s, syntax.LangBash)
	return out
}

// Get loads a job and works out its status.
func Get(id string) (*Job, error) {
	if !validID(id) {
		return nil, fmt.Errorf("invalid job id %q", id)
	}
	raw, err := os.ReadFile(filepath.Join(dir(id), "job.json"))
	if err != nil {
		return nil, fmt.Errorf("unknown job %q", id)
	}
	var j Job
	if err := json.Unmarshal(raw, &j); err != nil {
		return nil, err
	}
	j.Status = Running
	if b, err := os.ReadFile(filepath.Join(dir(id), "exit")); err == nil {
		code, _ := strconv.Atoi(strings.TrimSpace(string(b)))
		j.Exit = &code
		if fi, err := os.Stat(filepath.Join(dir(id), "exit")); err == nil {
			j.Finished = fi.ModTime()
		}
		switch {
		case code == 0:
			j.Status = Succeeded
		case fileExists(filepath.Join(dir(id), "cancelled")):
			j.Status = Cancelled
		default:
			j.Status = Failed
		}
	} else if !j.alive() {
		j.Status = Lost
		if fileExists(filepath.Join(dir(id), "cancelled")) {
			j.Status = Cancelled
		}
	}
	end := time.Now()
	if !j.Finished.IsZero() {
		end = j.Finished
	}
	j.Elapsed = end.Sub(j.Started).Round(time.Second).String()
	// Logs go to the agent only when something went wrong. A running or
	// successful build's output is thousands of lines the agent can't act on;
	// the log file is there (LogPath) if it does need it.
	switch j.Status {
	case Failed:
		j.Error = NixError(id)
		if j.Error == "" {
			j.Tail = Tail(id, 40)
		}
	case Lost:
		j.Tail = Tail(id, 40)
	}
	return &j, nil
}

func (j *Job) alive() bool {
	if j.Unit != "" {
		out, _ := exec.Command("systemctl", "--user", "is-active", j.Unit).Output()
		s := strings.TrimSpace(string(out))
		return s == "active" || s == "activating" || s == "deactivating"
	}
	if j.PID > 0 {
		return syscall.Kill(j.PID, 0) == nil
	}
	return false
}

// Done reports whether the job has finished in any way.
func (j *Job) Done() bool { return j.Status != Running }

// LogPath is the job's combined output.
func LogPath(id string) string { return filepath.Join(dir(id), "log") }

// Tail returns the last n lines of a job's log.
func Tail(id string, n int) string {
	b, err := os.ReadFile(LogPath(id))
	if err != nil {
		return ""
	}
	lines := strings.Split(strings.TrimRight(string(b), "\n"), "\n")
	if len(lines) > n {
		lines = lines[len(lines)-n:]
	}
	s := strings.Join(lines, "\n")
	if len(s) > 6000 {
		s = "…" + s[len(s)-6000:]
	}
	return s
}

// NixError extracts the last "error:" block from a job's log.
func NixError(id string) string {
	b, err := os.ReadFile(LogPath(id))
	if err != nil {
		return ""
	}
	lines := strings.Split(string(b), "\n")
	start := -1
	for i := len(lines) - 1; i >= 0; i-- {
		if strings.HasPrefix(strings.TrimSpace(lines[i]), "error:") {
			start = i
			// Walk back over a stack of "error:" / "… while" lines.
			for start > 0 && (strings.HasPrefix(strings.TrimSpace(lines[start-1]), "…") || strings.HasPrefix(strings.TrimSpace(lines[start-1]), "error:") || strings.TrimSpace(lines[start-1]) == "") && i-start < 60 {
				start--
			}
			break
		}
	}
	if start < 0 {
		return ""
	}
	s := strings.TrimSpace(strings.Join(lines[start:], "\n"))
	if len(s) > 4000 {
		s = s[:4000] + "\n…"
	}
	return s
}

// Await blocks until the job finishes, ctx ends or block elapses.
func Await(ctx context.Context, id string, block time.Duration) (*Job, error) {
	deadline := time.Now().Add(block)
	for {
		j, err := Get(id)
		if err != nil || j.Done() || time.Now().After(deadline) {
			return j, err
		}
		select {
		case <-ctx.Done():
			return j, nil
		case <-time.After(2 * time.Second):
		}
	}
}

// Cancel stops a running job.
func Cancel(id string) (*Job, error) {
	j, err := Get(id)
	if err != nil {
		return nil, err
	}
	if j.Done() {
		return j, nil
	}
	_ = os.WriteFile(filepath.Join(dir(id), "cancelled"), nil, 0o600)
	if j.Unit != "" {
		_ = exec.Command("systemctl", "--user", "stop", j.Unit).Run()
	} else if j.PID > 0 {
		_ = syscall.Kill(-j.PID, syscall.SIGTERM)
	}
	time.Sleep(time.Second)
	return Get(id)
}

// List returns recent jobs, newest first.
func List(limit int) []*Job {
	entries, _ := os.ReadDir(root())
	var out []*Job
	for _, e := range entries {
		if j, err := Get(e.Name()); err == nil {
			j.Tail = ""
			j.Error = ""
			out = append(out, j)
		}
	}
	sort.Slice(out, func(a, b int) bool { return out[a].Started.After(out[b].Started) })
	if limit > 0 && len(out) > limit {
		out = out[:limit]
	}
	return out
}

func fileExists(p string) bool {
	_, err := os.Stat(p)
	return err == nil
}
