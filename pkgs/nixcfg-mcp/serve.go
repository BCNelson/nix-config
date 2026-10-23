package main

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/modelcontextprotocol/go-sdk/mcp"

	"nixcfg-mcp/internal/app"
	"nixcfg-mcp/internal/humancmd"
	"nixcfg-mcp/internal/jobs"
	"nixcfg-mcp/internal/pipeline"
	"nixcfg-mcp/internal/secrets"
)

type server struct {
	app *app.App
	// cursors remembers how many events of each wait were already returned,
	// so a repeated wait_for only reports what is new.
	mu      sync.Mutex
	cursors map[string]int
}

func serve(ap *app.App) error {
	s := &server{app: ap, cursors: map[string]int{}}
	srv := mcp.NewServer(&mcp.Implementation{Name: "nixcfg", Version: version}, &mcp.ServerOptions{
		Instructions: "Deploy, CI and secrets tooling for the nix-config repo. Read AGENTS.md first. " +
			"To wait for anything (CI, auto-update, hosts applying a commit, builds) use wait_for / job_wait -- never sleep loops. " +
			"wait_for blocks up to block_sec and returns state=pending with a wait_id: call it again with that wait_id until the state is terminal. " +
			"Anything listed under human_actions is for the human: hand it over on one line.",
	})

	ms := make([]string, len(pipeline.AllMilestones))
	for i, m := range pipeline.AllMilestones {
		ms[i] = string(m)
	}
	milestones := strings.Join(ms, " -> ")

	mcp.AddTool(srv, &mcp.Tool{
		Name: "wait_for",
		Description: "Wait for a commit to reach a milestone of the deploy timeline (" + milestones + "; default live). " +
			"Walks every earlier stage and stops at the first that fails or can never happen, with the stage, reason, log tail and any human hand-off. " +
			"Blocks up to block_sec (default 240) then returns state=pending with wait_id; call again with wait_id (sha not needed) to keep waiting. " +
			"Terminal states: succeeded, partial (some hosts made it; the others were unreachable or did not finish before the deadline -- strict=true makes that a failure), failed, will_not_happen, blocked (needs the human), unreachable, timed_out.",
	}, s.waitFor)
	mcp.AddTool(srv, &mcp.Tool{Name: "wait_status", Description: "Current state of a wait without blocking."}, s.waitStatus)
	mcp.AddTool(srv, &mcp.Tool{Name: "wait_cancel", Description: "Cancel a wait."}, s.waitCancel)
	mcp.AddTool(srv, &mcp.Tool{Name: "waits_list", Description: "Recent waits and their states."}, s.waitsList)
	mcp.AddTool(srv, &mcp.Tool{
		Name:        "deploy_status",
		Description: "One look at where a commit (default HEAD) is on the deploy timeline, and why it hasn't deployed if it hasn't. Probes hosts when until is a host stage (the default).",
	}, s.deployStatus)
	mcp.AddTool(srv, &mcp.Tool{Name: "hosts_list", Description: "Hosts, their auto-update settings and whether nixcfg can observe them. probe=true also reads each observable host's live state."}, s.hostsList)
	mcp.AddTool(srv, &mcp.Tool{Name: "host_status", Description: "A host's checkout branch, auto-update state file, service result, pending reboot, plus a plain-language diagnosis and the last run's journal when it failed."}, s.hostStatus)
	mcp.AddTool(srv, &mcp.Tool{Name: "explain_change", Description: "Whether commits (default origin/main..HEAD plus the working tree; or a rev, or a..b) touch paths check.yml watches -- i.e. whether pushing them runs CI and deploys."}, s.explain)
	mcp.AddTool(srv, &mcp.Tool{Name: "check_host", Description: "Start a background job that evaluates (mode=eval), dry-runs (default) or builds (mode=build) a host's system; all=true dry-runs every host. Returns a job_id for job_wait."}, s.checkHost)
	mcp.AddTool(srv, &mcp.Tool{Name: "preseed", Description: "Start a background job that builds romeo-2 or whiskey-1 here and copies it into romeo's store, so auto-update doesn't rebuild it. Needs a clean checkout at the commit to be pushed, and git-crypt unlocked."}, s.preseed)
	mcp.AddTool(srv, &mcp.Tool{Name: "job_wait", Description: "Wait up to block_sec (default 240) for a background job; returns its status, exit code and the nix error or log tail. Call again while it is running."}, s.jobWait)
	mcp.AddTool(srv, &mcp.Tool{Name: "job_status", Description: "A background job's status and log tail, without blocking."}, s.jobStatus)
	mcp.AddTool(srv, &mcp.Tool{Name: "job_cancel", Description: "Stop a background job."}, s.jobCancel)
	mcp.AddTool(srv, &mcp.Tool{Name: "jobs_list", Description: "Recent background jobs."}, s.jobsList)
	mcp.AddTool(srv, &mcp.Tool{Name: "secrets_status", Description: "For a host (or all): declared agenix secrets vs rekeyed files in secrets/hosts/, without decrypting. Reports missing/stale/untracked secrets and who fixes each (agent or human, and whether it needs a key touch)."}, s.secretsStatus)
	mcp.AddTool(srv, &mcp.Tool{Name: "human_command", Description: "The exact one-line command to hand the human for an action: " + strings.Join(humancmd.ActionNames(), ", ") + "."}, s.human)

	return srv.Run(context.Background(), &mcp.StdioTransport{})
}

func text(s string) *mcp.CallToolResult {
	return &mcp.CallToolResult{Content: []mcp.Content{&mcp.TextContent{Text: s}}}
}

// ---- waits ------------------------------------------------------------------

type waitForIn struct {
	SHA        string   `json:"sha,omitempty" jsonschema:"commit or revision to follow (default HEAD); omit when passing wait_id"`
	Until      string   `json:"until,omitempty" jsonschema:"milestone to wait for (default live)"`
	Hosts      []string `json:"hosts,omitempty" jsonschema:"hosts for host stages; default: observable hosts that apply within minutes"`
	KeepGoing  bool     `json:"keep_going,omitempty" jsonschema:"keep waiting on other hosts after one fails"`
	Strict     bool     `json:"strict,omitempty" jsonschema:"treat an unreachable or timed-out host as a failure (default: keep going and end partial)"`
	TimeoutMin int      `json:"timeout_min,omitempty" jsonschema:"overall deadline in minutes (default 60)"`
	BlockSec   int      `json:"block_sec,omitempty" jsonschema:"how long this call blocks before returning pending (default 240, max 540)"`
	WaitID     string   `json:"wait_id,omitempty" jsonschema:"continue an existing wait"`
}

type waitOut struct {
	WaitID string           `json:"wait_id"`
	State  pipeline.State   `json:"state"`
	Result pipeline.Result  `json:"result"`
	Events []pipeline.Event `json:"new_events,omitempty"`
}

func (s *server) waitFor(ctx context.Context, req *mcp.CallToolRequest, in waitForIn) (*mcp.CallToolResult, waitOut, error) {
	var h *pipeline.Handle
	var err error
	if in.WaitID != "" {
		h, err = s.app.Store.Get(in.WaitID)
	} else {
		until, perr := pipeline.ParseMilestone(in.Until)
		if perr != nil {
			return nil, waitOut{}, perr
		}
		sha, rerr := s.app.ResolveRev(ctx, in.SHA)
		if rerr != nil {
			return nil, waitOut{}, rerr
		}
		timeout := time.Duration(in.TimeoutMin) * time.Minute
		var w *pipeline.Wait
		w, err = s.app.Store.Start(pipeline.Spec{SHA: sha, Until: until, Hosts: in.Hosts, KeepGoing: in.KeepGoing, Strict: in.Strict, Timeout: timeout})
		if err == nil {
			h, err = s.app.Store.Get(w.ID())
		}
	}
	if err != nil {
		return nil, waitOut{}, err
	}
	block := time.Duration(in.BlockSec) * time.Second
	if block <= 0 {
		block = 240 * time.Second
	}
	block = min(block, 540*time.Second)

	s.mu.Lock()
	cursor := s.cursors[h.ID]
	s.mu.Unlock()

	var latest pipeline.Event
	stop := progress(ctx, req, func() string {
		if latest.Text == "" {
			return "waiting"
		}
		return latest.Line()
	})
	r, next, err := h.Await(ctx, cursor, block, func(e pipeline.Event) { latest = e })
	stop()
	if err != nil {
		return nil, waitOut{}, err
	}
	_, evs, _ := h.Snapshot(cursor)
	if len(evs) > next-cursor {
		evs = evs[:next-cursor]
	}
	s.mu.Lock()
	s.cursors[h.ID] = next
	s.mu.Unlock()

	out := waitOut{WaitID: h.ID, State: r.State, Result: r, Events: evs}
	var b strings.Builder
	b.WriteString(app.RenderResult(&r))
	if len(evs) > 0 {
		b.WriteString("\nnew events:\n")
		b.WriteString(app.RenderEvents(evs))
	}
	if !r.State.Terminal() {
		fmt.Fprintf(&b, "\nstill pending: call wait_for with wait_id=%q to keep waiting.\n", h.ID)
	}
	return text(b.String()), out, nil
}

// progress sends a notification every few seconds while a call blocks, which
// keeps clients from timing the call out and shows what is happening.
func progress(ctx context.Context, req *mcp.CallToolRequest, msg func() string) func() {
	tok := req.Params.GetProgressToken()
	if tok == nil {
		return func() {}
	}
	ctx, cancel := context.WithCancel(ctx)
	go func() {
		t := time.NewTicker(5 * time.Second)
		defer t.Stop()
		var n float64
		for {
			select {
			case <-ctx.Done():
				return
			case <-t.C:
				n++
				_ = req.Session.NotifyProgress(ctx, &mcp.ProgressNotificationParams{ProgressToken: tok, Progress: n, Message: msg()})
			}
		}
	}()
	return cancel
}

type waitIDIn struct {
	WaitID string `json:"wait_id"`
}

func (s *server) waitStatus(_ context.Context, _ *mcp.CallToolRequest, in waitIDIn) (*mcp.CallToolResult, waitOut, error) {
	h, err := s.app.Store.Get(in.WaitID)
	if err != nil {
		return nil, waitOut{}, err
	}
	r, _, err := h.Snapshot(0)
	if err != nil {
		return nil, waitOut{}, err
	}
	return text(app.RenderResult(&r)), waitOut{WaitID: h.ID, State: r.State, Result: r}, nil
}

func (s *server) waitCancel(_ context.Context, _ *mcp.CallToolRequest, in waitIDIn) (*mcp.CallToolResult, waitOut, error) {
	h, err := s.app.Store.Get(in.WaitID)
	if err != nil {
		return nil, waitOut{}, err
	}
	if err := h.Cancel(); err != nil {
		return nil, waitOut{}, err
	}
	r, _, _ := h.Snapshot(0)
	return text(app.RenderResult(&r)), waitOut{WaitID: h.ID, State: r.State, Result: r}, nil
}

type waitsOut struct {
	Waits []pipeline.Result `json:"waits"`
}

func (s *server) waitsList(_ context.Context, _ *mcp.CallToolRequest, _ struct{}) (*mcp.CallToolResult, waitsOut, error) {
	rs := s.app.Store.List(20)
	var b strings.Builder
	for _, r := range rs {
		fmt.Fprintf(&b, "%s %-14s %s -> %s %s\n", r.ID, r.State, short(r.Spec.SHA), r.Spec.Until, r.Reason)
	}
	if b.Len() == 0 {
		b.WriteString("no waits\n")
	}
	return text(b.String()), waitsOut{Waits: rs}, nil
}

type statusIn struct {
	Rev   string   `json:"rev,omitempty" jsonschema:"commit or revision (default HEAD)"`
	Until string   `json:"until,omitempty" jsonschema:"milestone (default live; a repo stage skips probing hosts)"`
	Hosts []string `json:"hosts,omitempty"`
}

func (s *server) deployStatus(ctx context.Context, _ *mcp.CallToolRequest, in statusIn) (*mcp.CallToolResult, pipeline.Result, error) {
	until, err := pipeline.ParseMilestone(in.Until)
	if err != nil {
		return nil, pipeline.Result{}, err
	}
	r, _, err := s.app.Status(ctx, in.Rev, until, in.Hosts)
	if err != nil {
		return nil, pipeline.Result{}, err
	}
	return text(app.RenderResult(r)), *r, nil
}

// ---- hosts ------------------------------------------------------------------

type hostsIn struct {
	Probe bool `json:"probe,omitempty" jsonschema:"also probe observable hosts live"`
}

type hostsOut struct {
	Hosts []app.HostRow `json:"hosts"`
}

func (s *server) hostsList(ctx context.Context, _ *mcp.CallToolRequest, in hostsIn) (*mcp.CallToolResult, hostsOut, error) {
	rows := s.app.Hosts(ctx, in.Probe)
	return text(app.RenderHosts(rows)), hostsOut{Hosts: rows}, nil
}

type hostIn struct {
	Host string `json:"host" jsonschema:"flake host name (romeo-2) or short name (romeo)"`
	Log  bool   `json:"log,omitempty" jsonschema:"include the last run's journal even if it succeeded"`
}

func (s *server) hostStatus(ctx context.Context, _ *mcp.CallToolRequest, in hostIn) (*mcp.CallToolResult, app.HostReport, error) {
	r, err := s.app.HostStatus(ctx, in.Host, in.Log)
	if err != nil {
		return nil, app.HostReport{}, err
	}
	return text(app.RenderHostReport(r)), *r, nil
}

type explainIn struct {
	Rev string `json:"rev,omitempty" jsonschema:"a commit, or a range a..b; default origin/main..HEAD plus the working tree"`
}

func (s *server) explain(ctx context.Context, _ *mcp.CallToolRequest, in explainIn) (*mcp.CallToolResult, app.Explanation, error) {
	e, err := s.app.Explain(ctx, in.Rev)
	if err != nil {
		return nil, app.Explanation{}, err
	}
	return text(app.RenderExplanation(e)), *e, nil
}

// ---- jobs -------------------------------------------------------------------

type checkIn struct {
	Host string `json:"host,omitempty"`
	All  bool   `json:"all,omitempty" jsonschema:"dry-run every host (like CI's check-hosts)"`
	Mode string `json:"mode,omitempty" jsonschema:"eval | dry-run (default) | build"`
}

func (s *server) checkHost(_ context.Context, _ *mcp.CallToolRequest, in checkIn) (*mcp.CallToolResult, jobs.Job, error) {
	j, err := s.app.CheckHost(in.Host, in.Mode, in.All)
	if err != nil {
		return nil, jobs.Job{}, err
	}
	return text(app.RenderJob(j) + "follow with job_wait job_id=" + j.ID + "\n"), *j, nil
}

type preseedIn struct {
	Host string `json:"host" jsonschema:"romeo-2 or whiskey-1"`
}

func (s *server) preseed(ctx context.Context, _ *mcp.CallToolRequest, in preseedIn) (*mcp.CallToolResult, jobs.Job, error) {
	j, err := s.app.Preseed(ctx, in.Host)
	if err != nil {
		return nil, jobs.Job{}, err
	}
	return text(app.RenderJob(j) + "follow with job_wait job_id=" + j.ID + "\n"), *j, nil
}

type jobWaitIn struct {
	JobID    string `json:"job_id"`
	BlockSec int    `json:"block_sec,omitempty" jsonschema:"how long to block (default 240, max 540)"`
}

func (s *server) jobWait(ctx context.Context, req *mcp.CallToolRequest, in jobWaitIn) (*mcp.CallToolResult, jobs.Job, error) {
	block := time.Duration(in.BlockSec) * time.Second
	if block <= 0 {
		block = 240 * time.Second
	}
	block = min(block, 540*time.Second)
	stop := progress(ctx, req, func() string {
		t := jobs.Tail(in.JobID, 1)
		if t == "" {
			return "running"
		}
		return t
	})
	j, err := jobs.Await(ctx, in.JobID, block)
	stop()
	if err != nil {
		return nil, jobs.Job{}, err
	}
	return text(app.RenderJob(j)), *j, nil
}

type jobIn struct {
	JobID string `json:"job_id"`
}

func (s *server) jobStatus(_ context.Context, _ *mcp.CallToolRequest, in jobIn) (*mcp.CallToolResult, jobs.Job, error) {
	j, err := jobs.Get(in.JobID)
	if err != nil {
		return nil, jobs.Job{}, err
	}
	return text(app.RenderJob(j)), *j, nil
}

func (s *server) jobCancel(_ context.Context, _ *mcp.CallToolRequest, in jobIn) (*mcp.CallToolResult, jobs.Job, error) {
	j, err := jobs.Cancel(in.JobID)
	if err != nil {
		return nil, jobs.Job{}, err
	}
	return text(app.RenderJob(j)), *j, nil
}

type jobsOut struct {
	Jobs []*jobs.Job `json:"jobs"`
}

func (s *server) jobsList(_ context.Context, _ *mcp.CallToolRequest, _ struct{}) (*mcp.CallToolResult, jobsOut, error) {
	js := jobs.List(30)
	return text(app.RenderJobs(js)), jobsOut{Jobs: js}, nil
}

// ---- secrets / human ----------------------------------------------------------

type secretsIn struct {
	Host string `json:"host,omitempty" jsonschema:"a host, or all (default all; that evaluates every host and takes a few minutes)"`
}

type secretsOut struct {
	Reports []*secrets.Report `json:"reports"`
}

func (s *server) secretsStatus(ctx context.Context, _ *mcp.CallToolRequest, in secretsIn) (*mcp.CallToolResult, secretsOut, error) {
	reps, err := s.app.SecretsStatus(ctx, in.Host)
	if err != nil {
		return nil, secretsOut{}, err
	}
	return text(app.RenderSecrets(reps)), secretsOut{Reports: reps}, nil
}

type humanIn struct {
	Action string `json:"action"`
	Host   string `json:"host,omitempty"`
	Arg    string `json:"arg,omitempty" jsonschema:"extra argument, e.g. the .age path for edit-secret"`
}

func (s *server) human(_ context.Context, _ *mcp.CallToolRequest, in humanIn) (*mcp.CallToolResult, humancmd.Action, error) {
	a, err := humancmd.Build(s.app.Cfg, in.Action, in.Host, in.Arg)
	if err != nil {
		return nil, humancmd.Action{}, err
	}
	return text(app.RenderHuman(a)), a, nil
}
