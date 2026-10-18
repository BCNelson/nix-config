package pipeline

import (
	"errors"
	"strings"
	"testing"
	"time"

	"nixcfg-mcp/internal/config"
	"nixcfg-mcp/internal/forgejo"
	"nixcfg-mcp/internal/hostprobe"
)

var t0 = time.Date(2026, 10, 18, 12, 0, 0, 0, time.UTC)

const sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

func run(idx int, commit, status string, jobs ...forgejo.RunJob) RunObs {
	return RunObs{
		Run:  forgejo.Run{ID: int64(100 + idx), Index: idx, WorkflowID: "check.yml", Status: status, CommitSHA: commit, Event: "push", PrettyRef: "main", Stopped: t0.Add(-time.Hour)},
		Own:  commit == sha,
		Jobs: jobs,
	}
}

func job(name, status string) forgejo.RunJob {
	return forgejo.RunJob{Name: name, Status: status, Attempt: 1}
}

func TestEvalRepo(t *testing.T) {
	base := RepoObs{At: t0, Pushed: true, PushedSince: t0.Add(-time.Minute), WatchedAhead: true, NextBump: NextBump(t0), Deadline: t0.Add(time.Hour)}

	cases := []struct {
		name  string
		obs   func(o RepoObs) RepoObs
		stage Milestone
		state State
		want  string // substring of the stage detail
	}{
		{"not pushed", func(o RepoObs) RepoObs { o.Pushed = false; return o }, Pushed, Pending, "local"},
		{"already deployed", func(o RepoObs) RepoObs { o.InAutoUpdate = true; return o }, AutoUpdateAdvanced, Succeeded, "contains"},
		{"waiting for runner", func(o RepoObs) RepoObs { return o }, CIStarted, Pending, "waiting for a check.yml run"},
		{"unwatched, short deadline", func(o RepoObs) RepoObs { o.WatchedAhead = false; return o }, CIStarted, WillNotHappen, "touches no path"},
		{"unwatched, rides the bump", func(o RepoObs) RepoObs {
			o.WatchedAhead = false
			o.Deadline = o.NextBump.Add(2 * time.Hour)
			return o
		}, CIStarted, Pending, "flake bump"},
		{"bump running", func(o RepoObs) RepoObs { o.WatchedAhead = false; o.BumpRunning = true; return o }, CIStarted, Pending, "update.yml"},
		{"check-hosts running", func(o RepoObs) RepoObs {
			o.CheckRuns = []RunObs{run(5, sha, "running", job("check-hosts", "running"))}
			return o
		}, CheckHostsPassed, Pending, "running"},
		{"check-hosts failed", func(o RepoObs) RepoObs {
			r := run(5, sha, "failure", job("check-hosts", "failure"), job("check-scripts", "success"), job("auto-update", "skipped"))
			r.CheckLog = &forgejo.CheckLog{Failed: []string{"golf-3"}, Hosts: []forgejo.HostCheck{{Host: "golf-3", OK: false, Tail: "error: attribute missing"}}}
			o.CheckRuns = []RunObs{r}
			return o
		}, CheckHostsPassed, Failed, "golf-3"},
		{"own run failed, later run still going", func(o RepoObs) RepoObs {
			o.CheckRuns = []RunObs{
				run(5, sha, "failure", job("check-hosts", "failure")),
				run(6, "bbbb", "running", job("check-hosts", "running")),
			}
			return o
		}, CheckHostsPassed, Pending, "running"},
		{"superseded by a later passing run", func(o RepoObs) RepoObs {
			o.CheckRuns = []RunObs{
				run(5, sha, "failure", job("check-hosts", "failure")),
				run(6, "bbbb", "success", job("check-hosts", "success"), job("auto-update", "success")),
			}
			return o
		}, CIPassed, Succeeded, "later commit"},
		{"other job failed", func(o RepoObs) RepoObs {
			r := run(5, sha, "failure", job("check-hosts", "success"), job("check-scripts", "failure"), job("auto-update", "skipped"))
			r.FailJob = "check-scripts"
			o.CheckRuns = []RunObs{r}
			return o
		}, CIPassed, Failed, "check-scripts"},
		{"CI passed, branch catching up", func(o RepoObs) RepoObs {
			r := run(5, sha, "success", job("check-hosts", "success"))
			r.Run.Stopped = t0.Add(-time.Minute)
			o.CheckRuns = []RunObs{r}
			return o
		}, AutoUpdateAdvanced, Pending, "waiting"},
		{"CI passed long ago, branch moved off", func(o RepoObs) RepoObs {
			o.CheckRuns = []RunObs{run(5, sha, "success", job("check-hosts", "success"))}
			return o
		}, AutoUpdateAdvanced, Failed, "moved off"},
	}
	for _, c := range cases {
		v := EvalRepo(c.obs(base))
		st := v.Stages[c.stage]
		if st.State != c.state || !strings.Contains(st.Detail, c.want) {
			t.Errorf("%s: %s = %s %q, want %s ~%q", c.name, c.stage, st.State, st.Detail, c.state, c.want)
		}
	}
}

func TestEvalRepoFailureCarriesHostTail(t *testing.T) {
	r := run(5, sha, "failure", job("check-hosts", "failure"))
	r.CheckLog = &forgejo.CheckLog{Failed: []string{"golf-3"}, Hosts: []forgejo.HostCheck{{Host: "golf-3", Tail: "error: attribute 'x' missing"}}}
	v := EvalRepo(RepoObs{At: t0, Pushed: true, WatchedAhead: true, CheckRuns: []RunObs{r}})
	if !strings.Contains(v.LogTail, "attribute 'x' missing") || len(v.CI.FailedHosts) != 1 {
		t.Fatalf("tail=%q ci=%+v", v.LogTail, v.CI)
	}
}

var romeo = config.Host{Name: "romeo-2", SSH: "bcnelson@romeo.b.nel.family", Checkout: "/config", Interval: "1h", Ntfy: true, Reboot: true}

func state(mut func(*hostprobe.State)) *hostprobe.State {
	s := &hostprobe.State{
		Host: "romeo-2", Enabled: true, Branch: "auto-update", Unlocked: true, MaxRetries: 3, Reboot: true,
		Active: "inactive", Result: "success", Ntfy: true, Current: "/nix/store/a", Profile: "/nix/store/a",
		BootID: "boot1", SystemState: "running", NextTimer: t0.Add(30 * time.Minute), Invocation: "inv1",
	}
	if mut != nil {
		mut(s)
	}
	return s
}

func TestEvalHost(t *testing.T) {
	type step struct {
		at  time.Duration
		obs HostObs
	}
	cases := []struct {
		name   string
		host   config.Host
		target Milestone
		steps  []step
		state  State
		stage  Milestone
		want   string
	}{
		{"applied", romeo, HostApplied, []step{{0, HostObs{State: state(nil), BuiltContains: Yes, Advanced: true}}}, Succeeded, HostApplied, "applied"},
		{"live, no reboot needed", romeo, HostRebootedOK, []step{{0, HostObs{State: state(nil), BuiltContains: Yes, Advanced: true}}}, Succeeded, HostRebootedOK, "no reboot needed"},
		{"building", romeo, HostApplied, []step{{0, HostObs{State: state(func(s *hostprobe.State) {
			s.Active = "activating"
			s.AttemptCount = 1
		}), AttemptContains: Yes, HeadContains: Yes, Advanced: true}}}, Pending, HostBuilding, "attempt 1/3"},
		{"retries exhausted", romeo, HostApplied, []step{{0, HostObs{State: state(func(s *hostprobe.State) {
			s.Result = "exit-code"
			s.AttemptCount = 3
			s.AttemptCommit = "cccc"
		}), AttemptContains: Yes, AttemptIsTip: true, Advanced: true, JournalTail: "error: builder failed"}}}, Failed, HostBuilding, "won't retry"},
		{"retries exhausted, newer commit coming", romeo, HostApplied, []step{{0, HostObs{State: state(func(s *hostprobe.State) {
			s.Result = "exit-code"
			s.AttemptCount = 3
		}), AttemptContains: Yes, AttemptIsTip: false, Advanced: true}}}, Pending, HostBuilding, "newer commit"},
		{"wrong branch", romeo, HostApplied, []step{{0, HostObs{State: state(func(s *hostprobe.State) { s.Branch = "main" })}}}, WillNotHappen, HostFetched, "branch main"},
		{"unobservable", config.Host{Name: "sierra-2"}, HostApplied, []step{{0, HostObs{}}}, WillNotHappen, HostFetched, "unobservable"},
		{"not auto-updating", config.Host{Name: "wyse-1", AutoUpdate: new(bool)}, HostApplied, []step{{0, HostObs{}}}, WillNotHappen, HostFetched, "does not run"},
		{"brief outage", romeo, HostApplied, []step{
			{0, HostObs{Err: errors.New("dial timeout")}},
			{5 * time.Minute, HostObs{Err: errors.New("dial timeout")}},
		}, Pending, "", ""},
		{"unreachable", romeo, HostApplied, []step{
			{0, HostObs{Err: errors.New("dial timeout")}},
			{11 * time.Minute, HostObs{Err: errors.New("dial timeout")}},
		}, Unreachable, "", "unreachable for"},
		{"needs login, then deadline", romeo, HostApplied, []step{
			{0, HostObs{Err: hostprobe.ErrNeedsLogin, Deadline: t0.Add(time.Minute)}},
			{2 * time.Minute, HostObs{Err: hostprobe.ErrNeedsLogin, Deadline: t0.Add(time.Minute)}},
		}, Blocked, "", "browser login"},
		{"reboot cycle", romeo, HostRebootedOK, []step{
			{0, HostObs{State: state(func(s *hostprobe.State) { s.KernelChanged = true; s.RebootScheduled = true }), BuiltContains: Yes, Advanced: true}},
			{time.Minute, HostObs{Err: errors.New("connection refused")}},
			{3 * time.Minute, HostObs{State: state(func(s *hostprobe.State) { s.BootID = "boot2" }), BuiltContains: Yes, Advanced: true}},
		}, Succeeded, HostRebootedOK, "rebooted into it"},
		{"staged for boot, host won't reboot", config.Host{Name: "golf-2", SSH: "u@golf", Ntfy: true}, HostApplied, []step{
			{0, HostObs{State: state(func(s *hostprobe.State) { s.Profile = "/nix/store/b"; s.Reboot = false }), BuiltContains: Yes, Advanced: true}},
		}, WillNotHappen, HostApplied, "doesn't reboot itself"},
		{"no ntfy, timer after deadline", config.Host{Name: "qilin-1", SSH: "u@qilin", Interval: "24h"}, HostApplied, []step{
			{0, HostObs{State: state(func(s *hostprobe.State) { s.Ntfy = false; s.NextTimer = t0.Add(20 * time.Hour) }), Advanced: true, AdvancedAt: t0, Deadline: t0.Add(time.Hour)}},
		}, Blocked, HostFetched, "after the wait's deadline"},
		{"failed before building, twice", romeo, HostApplied, []step{
			{0, HostObs{State: state(func(s *hostprobe.State) { s.Result = "exit-code"; s.Invocation = "i1"; s.ExitedAt = t0 }), Advanced: true, AdvancedAt: t0.Add(-time.Minute)}},
			{time.Minute, HostObs{State: state(func(s *hostprobe.State) {
				s.Result = "exit-code"
				s.Invocation = "i2"
				s.ExitedAt = t0.Add(time.Minute)
			}), Advanced: true, AdvancedAt: t0.Add(-time.Minute)}},
		}, Failed, HostFetched, "before building"},
		{"git-crypt locked", romeo, HostApplied, []step{
			{0, HostObs{State: state(func(s *hostprobe.State) { s.Unlocked = false; s.Result = "exit-code" }), Advanced: true, AdvancedAt: t0}},
		}, Failed, HostFetched, "git-crypt"},
	}
	for _, c := range cases {
		p := &HostProgress{Host: c.host.Name, State: Pending}
		for _, s := range c.steps {
			o := s.obs
			o.Host = c.host
			o.Now = t0.Add(s.at)
			EvalHost(p, c.target, o)
		}
		if p.State != c.state || (c.stage != "" && p.Stage != c.stage) || !strings.Contains(p.Detail, c.want) {
			t.Errorf("%s: got %s at %s %q; want %s at %s ~%q", c.name, p.State, p.Stage, p.Detail, c.state, c.stage, c.want)
		}
	}
}

func TestSelectHosts(t *testing.T) {
	f := false
	cfg := &config.Config{Hosts: map[string]config.Host{
		"romeo-2": romeo,
		"ryuu-2":  {Name: "ryuu-2", SSH: "u@ryuu", Interval: "5m"},
		"qilin-1": {Name: "qilin-1", SSH: "u@qilin", Interval: "24h"},
		"golf-2":  {Name: "golf-2", Ntfy: true},
		"wyse-1":  {Name: "wyse-1", AutoUpdate: &f},
	}}
	sel, err := SelectHosts(cfg, nil)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Join(sel.Hosts, ",") != "romeo-2,ryuu-2" {
		t.Errorf("hosts = %v", sel.Hosts)
	}
	if sel.Excluded["golf-2"] != "unobservable" || !strings.Contains(sel.Excluded["qilin-1"], "24h") || sel.Excluded["wyse-1"] == "" {
		t.Errorf("excluded = %v", sel.Excluded)
	}
	sel, _ = SelectHosts(cfg, []string{"romeo"})
	if !sel.Explicit || sel.Hosts[0] != "romeo-2" {
		t.Errorf("explicit = %+v", sel)
	}
}

func TestNextBump(t *testing.T) {
	if got := NextBump(time.Date(2026, 10, 18, 8, 0, 0, 0, time.UTC)); got.Hour() != 9 || got.Day() != 18 {
		t.Errorf("before 9: %v", got)
	}
	if got := NextBump(time.Date(2026, 10, 18, 10, 0, 0, 0, time.UTC)); got.Day() != 19 {
		t.Errorf("after 9: %v", got)
	}
}
