// Command ssh-mcp is a minimal Model Context Protocol server that runs commands
// on an allow-list of hosts over SSH. It is purpose-built for Tailscale SSH:
// connections authenticate with the SSH "none" method (keyless), relying on the
// tailnet/WireGuard identity, so no keys or passwords are configured here.
//
// Hosts are declared on the command line as repeated --host name=user@addr
// flags. Each maps a friendly name to an SSH destination.
package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"sort"
	"strings"
	"time"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

type srv struct {
	hosts map[string]target
	jobs  *jobRegistry
}

func main() {
	log.SetFlags(0)
	log.SetPrefix("ssh-mcp: ")

	hosts, err := parseHosts(os.Args[1:])
	if err != nil {
		log.Fatal(err)
	}
	if len(hosts) == 0 {
		log.Fatal("no hosts configured; pass at least one --host name=user@addr")
	}

	s := &srv{hosts: hosts, jobs: newJobRegistry()}

	server := mcp.NewServer(&mcp.Implementation{Name: "ssh-mcp", Version: "0.1.0"}, nil)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "list_hosts",
		Description: "List the configured SSH hosts that can be targeted by the other tools.",
	}, s.listHosts)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "ssh_exec",
		Description: "Run a command on a configured host over SSH and wait for it to finish. Streams live output via progress notifications. Best for commands that complete in seconds to a few minutes; use ssh_start for longer work.",
	}, s.execTool)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "ssh_start",
		Description: "Start a command on a configured host in the background and return a job_id immediately. Use ssh_wait/ssh_status to follow it and ssh_cancel to stop it.",
	}, s.startTool)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "ssh_wait",
		Description: "Wait for a background job (from ssh_start) to finish, up to timeout_sec. Returns the final result if it completed, otherwise the current status and output-so-far (call again to keep waiting).",
	}, s.waitTool)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "ssh_status",
		Description: "Return the current status and buffered output of a background job without blocking.",
	}, s.statusTool)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "ssh_cancel",
		Description: "Cancel a running background job, terminating the remote command.",
	}, s.cancelTool)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "ssh_list_jobs",
		Description: "List all background jobs and their statuses.",
	}, s.listJobsTool)

	if err := server.Run(context.Background(), &mcp.StdioTransport{}); err != nil {
		log.Fatal(err)
	}
}

// ---- flag parsing ----------------------------------------------------------

// parseHosts reads repeated "--host name=user@addr" (or "--host=...") args.
func parseHosts(args []string) (map[string]target, error) {
	hosts := map[string]target{}
	for i := 0; i < len(args); i++ {
		a := args[i]
		var spec string
		switch {
		case a == "--host" || a == "-host":
			if i+1 >= len(args) {
				return nil, fmt.Errorf("missing value after %s", a)
			}
			i++
			spec = args[i]
		case strings.HasPrefix(a, "--host="):
			spec = strings.TrimPrefix(a, "--host=")
		case strings.HasPrefix(a, "-host="):
			spec = strings.TrimPrefix(a, "-host=")
		default:
			return nil, fmt.Errorf("unexpected argument %q", a)
		}

		tgt, err := parseHostSpec(spec)
		if err != nil {
			return nil, err
		}
		if _, dup := hosts[tgt.name]; dup {
			return nil, fmt.Errorf("duplicate host name %q", tgt.name)
		}
		hosts[tgt.name] = tgt
	}
	return hosts, nil
}

// parseHostSpec parses "name=user@host[:port]".
func parseHostSpec(spec string) (target, error) {
	name, rest, ok := strings.Cut(spec, "=")
	if !ok || name == "" {
		return target{}, fmt.Errorf("invalid --host %q, want name=user@host", spec)
	}
	user, addr, ok := strings.Cut(rest, "@")
	if !ok || user == "" || addr == "" {
		return target{}, fmt.Errorf("invalid --host %q, want name=user@host", spec)
	}
	if !strings.Contains(addr, ":") {
		addr += ":22"
	}
	return target{name: name, user: user, addr: addr}, nil
}

func (s *srv) hostNames() string {
	names := make([]string, 0, len(s.hosts))
	for n := range s.hosts {
		names = append(names, n)
	}
	sort.Strings(names)
	return strings.Join(names, ", ")
}

// ---- tool inputs/outputs ---------------------------------------------------

type listHostsOutput struct {
	Hosts []hostInfo `json:"hosts"`
}

type hostInfo struct {
	Name string `json:"name"`
	User string `json:"user"`
	Addr string `json:"addr"`
}

type execInput struct {
	Host       string `json:"host" jsonschema:"name of a configured host (see list_hosts)"`
	Command    string `json:"command" jsonschema:"shell command to run on the remote host"`
	TimeoutSec int    `json:"timeout_sec,omitempty" jsonschema:"max seconds to wait before giving up (default 300)"`
}

type execOutput struct {
	Host       string  `json:"host"`
	Command    string  `json:"command"`
	Status     string  `json:"status"` // exited | timeout | canceled | error
	ExitCode   *int    `json:"exit_code,omitempty"`
	Output     string  `json:"output"`
	Truncated  bool    `json:"truncated,omitempty"`
	TotalBytes int64   `json:"total_bytes"`
	ElapsedSec float64 `json:"elapsed_sec"`
	Error      string  `json:"error,omitempty"`
}

type startInput struct {
	Host    string `json:"host" jsonschema:"name of a configured host (see list_hosts)"`
	Command string `json:"command" jsonschema:"shell command to run on the remote host"`
}

type startOutput struct {
	JobID   string `json:"job_id"`
	Host    string `json:"host"`
	Command string `json:"command"`
	Status  string `json:"status"`
}

type waitInput struct {
	JobID      string `json:"job_id" jsonschema:"id returned by ssh_start"`
	TimeoutSec int    `json:"timeout_sec,omitempty" jsonschema:"max seconds to block waiting for completion (default 60)"`
}

type jobInput struct {
	JobID string `json:"job_id" jsonschema:"id returned by ssh_start"`
}

type jobOutput struct {
	JobID      string  `json:"job_id"`
	Host       string  `json:"host"`
	Command    string  `json:"command"`
	Status     string  `json:"status"`
	ExitCode   *int    `json:"exit_code,omitempty"`
	Output     string  `json:"output,omitempty"`
	Truncated  bool    `json:"truncated,omitempty"`
	TotalBytes int64   `json:"total_bytes"`
	ElapsedSec float64 `json:"elapsed_sec"`
	Error      string  `json:"error,omitempty"`
}

type listJobsOutput struct {
	Jobs []jobOutput `json:"jobs"`
}

// ---- tool handlers ---------------------------------------------------------

func (s *srv) listHosts(_ context.Context, _ *mcp.CallToolRequest, _ struct{}) (*mcp.CallToolResult, listHostsOutput, error) {
	names := make([]string, 0, len(s.hosts))
	for n := range s.hosts {
		names = append(names, n)
	}
	sort.Strings(names)

	out := listHostsOutput{}
	for _, n := range names {
		t := s.hosts[n]
		out.Hosts = append(out.Hosts, hostInfo{Name: t.name, User: t.user, Addr: t.addr})
	}
	return textResult(renderHosts(out)), out, nil
}

func (s *srv) execTool(ctx context.Context, req *mcp.CallToolRequest, in execInput) (*mcp.CallToolResult, execOutput, error) {
	tgt, ok := s.hosts[in.Host]
	if !ok {
		return nil, execOutput{}, fmt.Errorf("unknown host %q; configured hosts: %s", in.Host, s.hostNames())
	}

	timeout := 300 * time.Second
	if in.TimeoutSec > 0 {
		timeout = time.Duration(in.TimeoutSec) * time.Second
	}
	cctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	out := newOutputBuffer(maxOutput)
	start := time.Now()
	stop := s.streamProgress(cctx, req, out, start)
	code, err := runCommand(cctx, tgt, in.Command, out)
	stop()

	output, truncated, total := out.snapshot()
	res := execOutput{
		Host:       in.Host,
		Command:    in.Command,
		Output:     output,
		Truncated:  truncated,
		TotalBytes: total,
		ElapsedSec: time.Since(start).Seconds(),
	}
	switch {
	case err != nil && cctx.Err() == context.DeadlineExceeded && ctx.Err() == nil:
		res.Status = "timeout"
		res.Error = fmt.Sprintf("timed out after %s", timeout)
	case err != nil && ctx.Err() != nil:
		res.Status = "canceled"
	case err != nil:
		res.Status = "error"
		res.Error = err.Error()
	default:
		res.Status = "exited"
		res.ExitCode = &code
	}
	return textResult(renderExec(res)), res, nil
}

func (s *srv) startTool(_ context.Context, _ *mcp.CallToolRequest, in startInput) (*mcp.CallToolResult, startOutput, error) {
	tgt, ok := s.hosts[in.Host]
	if !ok {
		return nil, startOutput{}, fmt.Errorf("unknown host %q; configured hosts: %s", in.Host, s.hostNames())
	}
	// Use a background parent so the job survives this tool call returning.
	j := s.jobs.start(context.Background(), tgt, in.Command)
	out := startOutput{JobID: j.id, Host: in.Host, Command: in.Command, Status: string(statusRunning)}
	return textResult(fmt.Sprintf("started %s on %s: %s", out.JobID, out.Host, out.Command)), out, nil
}

func (s *srv) waitTool(ctx context.Context, req *mcp.CallToolRequest, in waitInput) (*mcp.CallToolResult, jobOutput, error) {
	j, ok := s.jobs.get(in.JobID)
	if !ok {
		return nil, jobOutput{}, fmt.Errorf("unknown job %q", in.JobID)
	}

	timeout := 60 * time.Second
	if in.TimeoutSec > 0 {
		timeout = time.Duration(in.TimeoutSec) * time.Second
	}

	stop := s.streamProgress(ctx, req, j.out, j.start)
	defer stop()

	timer := time.NewTimer(timeout)
	defer timer.Stop()
	select {
	case <-j.done:
	case <-timer.C:
	case <-ctx.Done():
	}

	out := toJobOutput(j.snapshot(), true)
	return textResult(renderJob(out)), out, nil
}

func (s *srv) statusTool(_ context.Context, _ *mcp.CallToolRequest, in jobInput) (*mcp.CallToolResult, jobOutput, error) {
	j, ok := s.jobs.get(in.JobID)
	if !ok {
		return nil, jobOutput{}, fmt.Errorf("unknown job %q", in.JobID)
	}
	out := toJobOutput(j.snapshot(), true)
	return textResult(renderJob(out)), out, nil
}

func (s *srv) cancelTool(_ context.Context, _ *mcp.CallToolRequest, in jobInput) (*mcp.CallToolResult, jobOutput, error) {
	j, ok := s.jobs.get(in.JobID)
	if !ok {
		return nil, jobOutput{}, fmt.Errorf("unknown job %q", in.JobID)
	}
	j.cancel()
	// Give the job a moment to unwind so the reported status reflects the cancel.
	select {
	case <-j.done:
	case <-time.After(5 * time.Second):
	}
	out := toJobOutput(j.snapshot(), true)
	return textResult(renderJob(out)), out, nil
}

func (s *srv) listJobsTool(_ context.Context, _ *mcp.CallToolRequest, _ struct{}) (*mcp.CallToolResult, listJobsOutput, error) {
	snaps := s.jobs.list()
	sort.Slice(snaps, func(i, j int) bool { return snaps[i].JobID < snaps[j].JobID })
	out := listJobsOutput{}
	for _, sn := range snaps {
		out.Jobs = append(out.Jobs, toJobOutput(sn, false))
	}
	return textResult(renderJobs(out)), out, nil
}

// ---- progress streaming ----------------------------------------------------

// streamProgress, if the request carries a progress token, starts a goroutine
// that emits ~1 Hz progress notifications carrying elapsed time and the tail of
// the output so far. The returned stop func ends the stream and must be called.
func (s *srv) streamProgress(ctx context.Context, req *mcp.CallToolRequest, out *outputBuffer, start time.Time) func() {
	tok := req.Params.GetProgressToken()
	if tok == nil {
		return func() {}
	}

	ctx, cancel := context.WithCancel(ctx)
	go func() {
		t := time.NewTicker(time.Second)
		defer t.Stop()
		var progress float64
		for {
			select {
			case <-ctx.Done():
				return
			case <-t.C:
				output, _, total := out.snapshot()
				progress++
				msg := fmt.Sprintf("[%.0fs] %d bytes\n%s",
					time.Since(start).Seconds(), total, tailLines(output, 5))
				_ = req.Session.NotifyProgress(ctx, &mcp.ProgressNotificationParams{
					ProgressToken: tok,
					Progress:      progress,
					Message:       msg,
				})
			}
		}
	}()
	return cancel
}

// ---- rendering helpers -----------------------------------------------------

func textResult(s string) *mcp.CallToolResult {
	return &mcp.CallToolResult{Content: []mcp.Content{&mcp.TextContent{Text: s}}}
}

func toJobOutput(s jobSnapshot, includeOutput bool) jobOutput {
	out := jobOutput{
		JobID:      s.JobID,
		Host:       s.Host,
		Command:    s.Command,
		Status:     string(s.Status),
		ExitCode:   s.ExitCode,
		Truncated:  s.Truncated,
		TotalBytes: s.TotalBytes,
		ElapsedSec: s.ElapsedSec,
		Error:      s.Error,
	}
	if includeOutput {
		out.Output = s.Output
	}
	return out
}

func renderHosts(o listHostsOutput) string {
	var b strings.Builder
	b.WriteString("configured hosts:\n")
	for _, h := range o.Hosts {
		fmt.Fprintf(&b, "  %s -> %s@%s\n", h.Name, h.User, h.Addr)
	}
	return strings.TrimRight(b.String(), "\n")
}

func renderExec(o execOutput) string {
	var b strings.Builder
	fmt.Fprintf(&b, "host=%s status=%s", o.Host, o.Status)
	if o.ExitCode != nil {
		fmt.Fprintf(&b, " exit=%d", *o.ExitCode)
	}
	fmt.Fprintf(&b, " elapsed=%.1fs\n", o.ElapsedSec)
	if o.Error != "" {
		fmt.Fprintf(&b, "error: %s\n", o.Error)
	}
	b.WriteString(o.Output)
	if o.Truncated {
		fmt.Fprintf(&b, "\n[output truncated; %d bytes total]", o.TotalBytes)
	}
	return b.String()
}

func renderJob(o jobOutput) string {
	var b strings.Builder
	fmt.Fprintf(&b, "%s host=%s status=%s", o.JobID, o.Host, o.Status)
	if o.ExitCode != nil {
		fmt.Fprintf(&b, " exit=%d", *o.ExitCode)
	}
	fmt.Fprintf(&b, " elapsed=%.1fs\n", o.ElapsedSec)
	if o.Error != "" {
		fmt.Fprintf(&b, "error: %s\n", o.Error)
	}
	b.WriteString(o.Output)
	if o.Truncated {
		fmt.Fprintf(&b, "\n[output truncated; %d bytes total]", o.TotalBytes)
	}
	return b.String()
}

func renderJobs(o listJobsOutput) string {
	if len(o.Jobs) == 0 {
		return "no jobs"
	}
	var b strings.Builder
	for _, j := range o.Jobs {
		fmt.Fprintf(&b, "%s host=%s status=%s elapsed=%.1fs", j.JobID, j.Host, j.Status, j.ElapsedSec)
		if j.ExitCode != nil {
			fmt.Fprintf(&b, " exit=%d", *j.ExitCode)
		}
		b.WriteString("\n")
	}
	return strings.TrimRight(b.String(), "\n")
}

// tailLines returns the last n non-empty-trimmed lines of s.
func tailLines(s string, n int) string {
	s = strings.TrimRight(s, "\n")
	if s == "" {
		return ""
	}
	lines := strings.Split(s, "\n")
	if len(lines) > n {
		lines = lines[len(lines)-n:]
	}
	return strings.Join(lines, "\n")
}
