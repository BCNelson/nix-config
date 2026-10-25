// Package pipeline models a deploy as one ordered timeline per commit and
// waits for a named milestone on it, walking every earlier stage and failing
// fast -- with the stage, a reason and a log tail -- when any of them can't
// succeed.
//
//	pushed -> ci_started -> check_hosts_passed -> ci_passed -> auto_update_advanced
//	  -> host_fetched -> host_building -> host_applied -> host_rebooted_ok
//
// "live" waits for host_applied on every selected host, plus host_rebooted_ok
// where a reboot is needed; "all_hosts_applied" stops at host_applied.
//
// Each stage is judged from outside state only (git refs, Forgejo, a probe of
// the host), so a wait carries no hidden state and can resume after restart.
package pipeline

import (
	"fmt"
	"slices"
	"strings"
	"time"

	"nixcfg-mcp/internal/forgejo"
)

type Milestone string

const (
	Pushed             Milestone = "pushed"
	CIStarted          Milestone = "ci_started"
	CheckHostsPassed   Milestone = "check_hosts_passed"
	CIPassed           Milestone = "ci_passed"
	AutoUpdateAdvanced Milestone = "auto_update_advanced"
	HostFetched        Milestone = "host_fetched"
	HostBuilding       Milestone = "host_building"
	HostApplied        Milestone = "host_applied"
	HostRebootedOK     Milestone = "host_rebooted_ok"
	AllHostsApplied    Milestone = "all_hosts_applied"
	Live               Milestone = "live"
)

// RepoStages are the stages before any host is involved, in order.
var RepoStages = []Milestone{Pushed, CIStarted, CheckHostsPassed, CIPassed, AutoUpdateAdvanced}

// HostStages are per-host stages, in order.
var HostStages = []Milestone{HostFetched, HostBuilding, HostApplied, HostRebootedOK}

// AllMilestones in timeline order, for help text and validation.
var AllMilestones = append(append(append([]Milestone{}, RepoStages...), HostStages...), AllHostsApplied, Live)

// ParseMilestone validates a milestone name.
func ParseMilestone(s string) (Milestone, error) {
	if s == "" {
		return Live, nil
	}
	for _, m := range AllMilestones {
		if string(m) == s {
			return m, nil
		}
	}
	names := make([]string, len(AllMilestones))
	for i, m := range AllMilestones {
		names[i] = string(m)
	}
	return "", fmt.Errorf("unknown milestone %q; one of: %s", s, strings.Join(names, ", "))
}

// IsHostStage reports whether reaching m involves hosts.
func (m Milestone) IsHostStage() bool {
	switch m {
	case HostFetched, HostBuilding, HostApplied, HostRebootedOK, AllHostsApplied, Live:
		return true
	}
	return false
}

// HostTarget is the per-host stage a host-level milestone requires.
func (m Milestone) HostTarget() Milestone {
	switch m {
	case AllHostsApplied:
		return HostApplied
	case Live:
		return HostRebootedOK // only when a reboot is needed; see hosts.go
	}
	return m
}

func repoIndex(m Milestone) int {
	for i, s := range RepoStages {
		if s == m {
			return i
		}
	}
	return len(RepoStages) // host stages come after every repo stage
}

func hostIndex(m Milestone) int {
	for i, s := range HostStages {
		if s == m {
			return i
		}
	}
	return -1
}

// State is where a stage, a host or a whole wait stands.
type State string

const (
	Pending       State = "pending"
	Succeeded     State = "succeeded"
	Failed        State = "failed"
	WillNotHappen State = "will_not_happen"
	Blocked       State = "blocked"
	Unreachable   State = "unreachable"
	TimedOut      State = "timed_out"
	// Partial: every host that could be reached got there; the rest were
	// unreachable or ran out of time. Only a whole wait ends this way.
	Partial State = "partial"
)

// Terminal reports whether s ends a wait.
func (s State) Terminal() bool { return s != Pending && s != "" }

// Soft reports whether a host ending this way says nothing about the change
// itself -- the host was just out of reach -- so it shouldn't stop a wait on
// the other hosts (unless the wait is strict).
func (s State) Soft() bool { return s == Unreachable || s == TimedOut }

// ExitCode is the CLI exit status for a terminal state.
func (s State) ExitCode() int {
	switch s {
	case Succeeded:
		return 0
	case Failed:
		return 1
	case TimedOut:
		return 3
	case Unreachable:
		return 4
	case WillNotHappen:
		return 5
	case Blocked:
		return 6
	case Partial:
		return 7
	}
	return 2
}

// Stage is one row of the timeline.
type Stage struct {
	Name       Milestone `json:"name"`
	State      State     `json:"state"`
	Detail     string    `json:"detail,omitempty"`
	StartedAt  time.Time `json:"started_at,omitempty"`
	FinishedAt time.Time `json:"finished_at,omitempty"`
}

// HostProgress is one host's position on the timeline.
type HostProgress struct {
	Host    string                  `json:"host"`
	Stage   Milestone               `json:"stage"` // the furthest stage reached
	State   State                   `json:"state"` // pending until the host's target is met or it can't be
	Detail  string                  `json:"detail,omitempty"`
	ETA     string                  `json:"eta,omitempty"`
	LogTail string                  `json:"log_tail,omitempty"`
	Human   []string                `json:"human_actions,omitempty"`
	Stages  map[Milestone]time.Time `json:"stages,omitempty"` // when each stage was first seen
	// bookkeeping across observations
	FailStreakSince time.Time `json:"fail_streak_since,omitempty"`
	LastError       string    `json:"last_error,omitempty"`
	BaseBootID      string    `json:"base_boot_id,omitempty"`
	FailedRuns      []string  `json:"failed_runs,omitempty"` // invocation ids that failed before building
	SeenInvocation  string    `json:"seen_invocation,omitempty"`
	RebootExpected  bool      `json:"reboot_expected,omitempty"`
}

// HostSelection records which hosts a wait covers and why.
type HostSelection struct {
	Hosts    []string          `json:"hosts"`
	Explicit bool              `json:"explicit"`
	Excluded map[string]string `json:"excluded,omitempty"` // host -> reason
}

// Event is one line of progress.
type Event struct {
	At    time.Time `json:"at"`
	Stage Milestone `json:"stage"`
	Host  string    `json:"host,omitempty"`
	Kind  string    `json:"kind"` // ok | progress | warn | fail | blocked | info
	Text  string    `json:"text"`
}

func (e Event) Line() string {
	var b strings.Builder
	b.WriteString(e.At.UTC().Format(time.RFC3339))
	b.WriteString(" ")
	if e.Stage == "" {
		b.WriteString("-")
	}
	b.WriteString(string(e.Stage))
	if e.Host != "" {
		b.WriteString(" ")
		b.WriteString(e.Host)
	}
	b.WriteString(" ")
	b.WriteString(e.Kind)
	b.WriteString(" ")
	b.WriteString(e.Text)
	return b.String()
}

// Spec is what to wait for.
type Spec struct {
	SHA       string    `json:"sha"`
	Until     Milestone `json:"until"`
	Hosts     []string  `json:"hosts,omitempty"`
	KeepGoing bool      `json:"keep_going,omitempty"`
	// Strict makes an unreachable or timed-out host fail the wait like any
	// other failure, instead of ending it as partial.
	Strict  bool          `json:"strict,omitempty"`
	Timeout time.Duration `json:"timeout"`
}

// Result is the whole wait, as reported to agents.
type Result struct {
	ID            string                   `json:"wait_id"`
	Spec          Spec                     `json:"spec"`
	Subject       string                   `json:"subject,omitempty"`
	State         State                    `json:"state"`
	CurrentStage  Milestone                `json:"current_stage"`
	FailedStage   Milestone                `json:"failed_stage,omitempty"`
	FailedHost    string                   `json:"failed_host,omitempty"`
	Reason        string                   `json:"reason,omitempty"`
	LogTail       string                   `json:"log_tail,omitempty"`
	HumanActions  []string                 `json:"human_actions,omitempty"`
	Stages        []Stage                  `json:"stages"`
	Hosts         map[string]*HostProgress `json:"hosts,omitempty"`
	HostSelection *HostSelection           `json:"host_selection,omitempty"`
	CI            *CIInfo                  `json:"ci,omitempty"`
	Created       time.Time                `json:"created"`
	Deadline      time.Time                `json:"deadline"`
	Updated       time.Time                `json:"updated"`
	Finished      time.Time                `json:"finished,omitempty"`
	EventCount    int                      `json:"event_count"`
}

// CIInfo summarises the CI run(s) that carry the commit.
type CIInfo struct {
	Run         int      `json:"run,omitempty"` // index_in_repo
	RunURL      string   `json:"run_url,omitempty"`
	RunCommit   string   `json:"run_commit,omitempty"`
	Superseded  bool     `json:"superseded,omitempty"` // the run is for a later commit containing sha
	Route       string   `json:"route,omitempty"`      // check.yml | update.yml | none
	FailedHosts []string `json:"failed_hosts,omitempty"`
	// While the run is going: when it started, and how long recent
	// successful runs of the workflow took (median). Remaining is worked out
	// when rendered, not stored, so it doesn't change on every poll.
	Started    time.Time `json:"started,omitzero"`
	TypicalSec int       `json:"typical_sec,omitempty"`
}

// Remaining estimates how much longer the run will take: "~3m20s left", or
// "longer than usual (+1m5s)" once it has run past the typical duration.
// Empty without enough history or once the run is done.
func (c *CIInfo) Remaining(now time.Time) string {
	if c == nil || c.Started.IsZero() || c.TypicalSec == 0 {
		return ""
	}
	left := c.Started.Add(time.Duration(c.TypicalSec) * time.Second).Sub(now).Round(time.Second)
	if left < 0 {
		return fmt.Sprintf("longer than usual (+%s)", -left)
	}
	return fmt.Sprintf("~%s left", left)
}

// typicalRuns is how many recent successful runs the estimate is based on.
const typicalRuns = 5

// TypicalDuration is the median duration of the last typicalRuns successful
// runs of workflow on main (runs newest first), or 0 with fewer than two.
// The median, so one slow run (a cold cache, a busy runner) doesn't drag the
// estimate; failed and cancelled runs stop early and would make it optimistic.
func TypicalDuration(runs []forgejo.Run, workflow string) time.Duration {
	var ds []time.Duration
	for _, r := range runs {
		if len(ds) == typicalRuns {
			break
		}
		if r.WorkflowID != workflow || r.Status != "success" || r.PrettyRef != "main" {
			continue
		}
		// Queued runs report started as the Unix epoch, not as unset.
		if r.Started.Unix() <= 0 || !r.Stopped.After(r.Started) {
			continue
		}
		ds = append(ds, r.Stopped.Sub(r.Started))
	}
	if len(ds) < 2 {
		return 0
	}
	slices.Sort(ds)
	if len(ds)%2 == 1 {
		return ds[len(ds)/2]
	}
	return (ds[len(ds)/2-1] + ds[len(ds)/2]) / 2
}

// Summary is a short human-readable final line.
func (r *Result) Summary() string {
	var b strings.Builder
	fmt.Fprintf(&b, "RESULT state=%s until=%s sha=%s", r.State, r.Spec.Until, short(r.Spec.SHA))
	if r.FailedStage != "" {
		fmt.Fprintf(&b, " failed_stage=%s", r.FailedStage)
	}
	if r.FailedHost != "" {
		fmt.Fprintf(&b, " host=%s", r.FailedHost)
	}
	if r.Reason != "" {
		fmt.Fprintf(&b, " reason=%q", r.Reason)
	}
	return b.String()
}

func short(s string) string {
	if len(s) > 10 {
		return s[:10]
	}
	return s
}
