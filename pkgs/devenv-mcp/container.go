package main

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"os/exec"
	"strings"
	"sync"
)

// ContainerID is an opaque identifier for a managed container.
type ContainerID string

// ContainerProvider abstracts container runtime operations so that different
// backends (Podman, Docker, etc.) can be swapped in.
type ContainerProvider interface {
	EnsureVolume(ctx context.Context, name string) error
	Create(ctx context.Context, opts CreateOpts) (ContainerID, error)
	Start(ctx context.Context, id ContainerID) error
	Stop(ctx context.Context, id ContainerID) error
	Remove(ctx context.Context, id ContainerID) error
	List(ctx context.Context, labelFilter map[string]string) ([]ContainerInfo, error)
	Exec(ctx context.Context, id ContainerID, cmd []string, opts ExecOpts) (int, string, error)
	// ExecStream runs cmd and writes combined stdout and stderr to out as the
	// command produces it, rather than returning everything at completion.
	// Background jobs use it to expose output while they are still running.
	ExecStream(ctx context.Context, id ContainerID, cmd []string, opts ExecOpts, out io.Writer) (int, error)
	WriteFile(ctx context.Context, id ContainerID, path string, content []byte) error
	ReadFile(ctx context.Context, id ContainerID, path string) ([]byte, error)
	IsRunning(ctx context.Context, id ContainerID) (bool, error)
}

// VolumeMount describes a named volume or bind mount.
type VolumeMount struct {
	Source string // named volume or host path
	Target string // mount point inside the container
}

// CreateOpts configures a new container.
type CreateOpts struct {
	Image   string
	Name    string
	Labels  map[string]string
	Env     map[string]string
	Volumes []VolumeMount
}

// ExecOpts configures command execution inside a container.
type ExecOpts struct {
	WorkDir string
	Env     map[string]string
	Stdin   io.Reader
}

// ContainerInfo describes a running or stopped container.
type ContainerInfo struct {
	ID     ContainerID
	Name   string
	Status string
	Labels map[string]string
}

// maxExecOutput caps how much output we retain from a single exec call.
const maxExecOutput = 256 * 1024

// outputBuffer is a concurrency-safe, size-bounded writer that retains the
// tail of the stream when the cap is exceeded.
type outputBuffer struct {
	mu        sync.Mutex
	buf       []byte
	cap       int
	total     int64
	truncated bool
}

func newOutputBuffer(cap int) *outputBuffer {
	return &outputBuffer{cap: cap}
}

func (b *outputBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.total += int64(len(p))
	b.buf = append(b.buf, p...)
	if len(b.buf) > b.cap {
		b.truncated = true
		b.buf = b.buf[len(b.buf)-b.cap:]
	}
	return len(p), nil
}

func (b *outputBuffer) snapshot() (string, bool, int64) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return string(b.buf), b.truncated, b.total
}

// PodmanProvider implements ContainerProvider using the podman CLI.
type PodmanProvider struct {
	// PodmanPath is the path to the podman binary.
	PodmanPath string
}

func (p *PodmanProvider) podman() string {
	if p.PodmanPath != "" {
		return p.PodmanPath
	}
	return "podman"
}

// EnsureVolume creates a named volume if it does not already exist.
func (p *PodmanProvider) EnsureVolume(ctx context.Context, name string) error {
	// "podman volume exists" returns 0 if the volume exists, 1 otherwise.
	check := exec.CommandContext(ctx, p.podman(), "volume", "exists", name)
	if check.Run() == nil {
		return nil // already exists
	}
	return p.run(ctx, "volume", "create", name)
}

func (p *PodmanProvider) Create(ctx context.Context, opts CreateOpts) (ContainerID, error) {
	args := []string{"create", "--name", opts.Name}
	for k, v := range opts.Labels {
		args = append(args, "--label", k+"="+v)
	}
	for k, v := range opts.Env {
		args = append(args, "--env", k+"="+v)
	}
	for _, v := range opts.Volumes {
		args = append(args, "--volume", v.Source+":"+v.Target)
	}
	args = append(args, opts.Image, "sleep", "infinity")

	var stdout, stderr bytes.Buffer
	cmd := exec.CommandContext(ctx, p.podman(), args...)
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("podman create: %w: %s", err, stderr.String())
	}
	return ContainerID(strings.TrimSpace(stdout.String())), nil
}

func (p *PodmanProvider) Start(ctx context.Context, id ContainerID) error {
	return p.run(ctx, "start", string(id))
}

func (p *PodmanProvider) Stop(ctx context.Context, id ContainerID) error {
	return p.run(ctx, "stop", "--time", "10", string(id))
}

func (p *PodmanProvider) Remove(ctx context.Context, id ContainerID) error {
	return p.run(ctx, "rm", "--force", string(id))
}

func (p *PodmanProvider) List(ctx context.Context, labelFilter map[string]string) ([]ContainerInfo, error) {
	args := []string{"ps", "-a", "--format", "{{.ID}}|{{.Names}}|{{.Status}}|{{.Labels}}", "--no-trunc"}
	for k, v := range labelFilter {
		args = append(args, "--filter", "label="+k+"="+v)
	}

	var stdout, stderr bytes.Buffer
	cmd := exec.CommandContext(ctx, p.podman(), args...)
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		return nil, fmt.Errorf("podman ps: %w: %s", err, stderr.String())
	}

	var containers []ContainerInfo
	for _, line := range strings.Split(strings.TrimSpace(stdout.String()), "\n") {
		if line == "" {
			continue
		}
		parts := strings.SplitN(line, "|", 4)
		if len(parts) < 3 {
			continue
		}
		labels := map[string]string{}
		if len(parts) == 4 {
			labels = parseLabels(parts[3])
		}
		containers = append(containers, ContainerInfo{
			ID:     ContainerID(parts[0]),
			Name:   parts[1],
			Status: parts[2],
			Labels: labels,
		})
	}
	return containers, nil
}

func (p *PodmanProvider) Exec(ctx context.Context, id ContainerID, cmd []string, opts ExecOpts) (int, string, error) {
	// stdout and stderr share one bounded buffer so interleaved output keeps
	// its relative order and a runaway command cannot exhaust memory.
	out := newOutputBuffer(maxExecOutput)
	code, err := p.ExecStream(ctx, id, cmd, opts, out)
	return code, renderOutput(out), err
}

func (p *PodmanProvider) ExecStream(ctx context.Context, id ContainerID, cmd []string, opts ExecOpts, out io.Writer) (int, error) {
	args := []string{"exec"}
	if opts.WorkDir != "" {
		args = append(args, "--workdir", opts.WorkDir)
	}
	for k, v := range opts.Env {
		args = append(args, "--env", k+"="+v)
	}
	args = append(args, string(id))
	args = append(args, cmd...)

	// Passing one writer for both streams keeps os/exec on a single pipe, so
	// writes stay ordered and never overlap.
	c := exec.CommandContext(ctx, p.podman(), args...)
	c.Stdout = out
	c.Stderr = out
	if opts.Stdin != nil {
		c.Stdin = opts.Stdin
	}

	if err := c.Run(); err != nil {
		if ee, ok := err.(*exec.ExitError); ok {
			return ee.ExitCode(), nil
		}
		return -1, err
	}
	return 0, nil
}

// renderOutput returns the buffered output, prefixed with a notice when the
// head of the stream was dropped to stay under the cap.
func renderOutput(b *outputBuffer) string {
	text, truncated, total := b.snapshot()
	if !truncated {
		return text
	}
	return fmt.Sprintf("[output truncated: showing last %d of %d bytes]\n%s", len(text), total, text)
}

func (p *PodmanProvider) WriteFile(ctx context.Context, id ContainerID, path string, content []byte) error {
	q := shellQuote(path)
	args := []string{"exec", "-i", string(id), "sh", "-c", fmt.Sprintf("mkdir -p \"$(dirname %s)\" && cat > %s", q, q)}
	c := exec.CommandContext(ctx, p.podman(), args...)
	c.Stdin = bytes.NewReader(content)
	var stderr bytes.Buffer
	c.Stderr = &stderr
	if err := c.Run(); err != nil {
		return fmt.Errorf("write file: %w: %s", err, stderr.String())
	}
	return nil
}

func (p *PodmanProvider) ReadFile(ctx context.Context, id ContainerID, path string) ([]byte, error) {
	args := []string{"exec", string(id), "cat", path}
	var stdout, stderr bytes.Buffer
	c := exec.CommandContext(ctx, p.podman(), args...)
	c.Stdout = &stdout
	c.Stderr = &stderr
	if err := c.Run(); err != nil {
		return nil, fmt.Errorf("read file: %w: %s", err, stderr.String())
	}
	return stdout.Bytes(), nil
}

func (p *PodmanProvider) IsRunning(ctx context.Context, id ContainerID) (bool, error) {
	args := []string{"inspect", "--format", "{{.State.Running}}", string(id)}
	var stdout, stderr bytes.Buffer
	c := exec.CommandContext(ctx, p.podman(), args...)
	c.Stdout = &stdout
	c.Stderr = &stderr
	if err := c.Run(); err != nil {
		return false, fmt.Errorf("inspect: %w: %s", err, stderr.String())
	}
	return strings.TrimSpace(stdout.String()) == "true", nil
}

// shellQuote wraps s in single quotes so it is safe to interpolate into a
// `sh -c` script, escaping any embedded single quotes.
func shellQuote(s string) string {
	return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'"
}

// parseLabels reads a container's labels from a podman `{{.Labels}}` field.
// Podman renders a Go map ("map[a:b c:d]") while Docker-compatible output uses
// "a=b,c=d"; both are accepted.
func parseLabels(s string) map[string]string {
	labels := map[string]string{}
	if inner, ok := strings.CutPrefix(s, "map["); ok {
		inner = strings.TrimSuffix(inner, "]")
		for _, kv := range strings.Fields(inner) {
			if k, v, ok := strings.Cut(kv, ":"); ok {
				labels[k] = v
			}
		}
		return labels
	}
	for _, kv := range strings.Split(s, ",") {
		if k, v, ok := strings.Cut(kv, "="); ok {
			labels[strings.TrimSpace(k)] = v
		}
	}
	return labels
}

func (p *PodmanProvider) run(ctx context.Context, args ...string) error {
	var stderr bytes.Buffer
	cmd := exec.CommandContext(ctx, p.podman(), args...)
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("podman %s: %w: %s", args[0], err, stderr.String())
	}
	return nil
}
