package pipeline

import (
	"fmt"
	"strings"
	"time"

	"nixcfg-mcp/internal/forgejo"
)

// RunObs is one CI run that may carry the commit.
type RunObs struct {
	Run      forgejo.Run       `json:"run"`
	Own      bool              `json:"own"` // the run is for sha itself, not a descendant
	Jobs     []forgejo.RunJob  `json:"jobs,omitempty"`
	CheckLog *forgejo.CheckLog `json:"check_log,omitempty"` // parsed for a failed check-hosts
	FailTail string            `json:"fail_tail,omitempty"` // tail of the first failed job's log
	FailJob  string            `json:"fail_job,omitempty"`
}

func (r RunObs) job(name string) (forgejo.RunJob, bool) {
	for _, j := range r.Jobs {
		if j.Name == name {
			return j, true
		}
	}
	return forgejo.RunJob{}, false
}

func jobDone(status string) bool {
	switch status {
	case "success", "failure", "cancelled", "skipped":
		return true
	}
	return false
}

// RepoObs is one observation of everything before the hosts.
type RepoObs struct {
	At            time.Time
	Pushed        bool      // sha is in origin/main
	PushedSince   time.Time // when we first saw it pushed
	InAutoUpdate  bool      // sha is in origin/auto-update
	MainSHA       string
	AutoUpdateSHA string
	// check.yml push runs on main whose commit contains sha, oldest first.
	CheckRuns []RunObs
	// A daily flake bump (update.yml) is running from a main that contains sha.
	BumpRunning bool
	// sha, or a commit after it on main, touches a path check.yml watches.
	WatchedAhead bool
	OwnWatched   []string // the watched paths sha itself touches
	NextBump     time.Time
	Deadline     time.Time
	Errors       []string // transient observation errors (progress only)
}

// RepoVerdict is the evaluation of the repo stages.
type RepoVerdict struct {
	Stages  map[Milestone]Stage
	CI      CIInfo
	Human   []string
	LogTail string
}

// EvalRepo judges the repo stages from one observation.
func EvalRepo(o RepoObs) RepoVerdict {
	v := RepoVerdict{Stages: map[Milestone]Stage{}}
	set := func(m Milestone, s State, detail string) {
		v.Stages[m] = Stage{Name: m, State: s, Detail: detail}
	}

	// pushed
	if o.Pushed {
		set(Pushed, Succeeded, "in origin/main")
	} else {
		set(Pushed, Pending, "only in the local checkout; push it: git push origin main")
		for _, m := range RepoStages[1:] {
			set(m, Pending, "")
		}
		return v
	}

	// Already deployed to the branch: everything before it is moot.
	if o.InAutoUpdate {
		detail := "auto-update (" + short(o.AutoUpdateSHA) + ") contains it"
		if run, ok := latestSuccess(o.CheckRuns); ok {
			v.CI = ciInfo(run, "check.yml")
			set(CIStarted, Succeeded, fmt.Sprintf("run #%d", run.Run.Index))
			set(CheckHostsPassed, Succeeded, fmt.Sprintf("run #%d", run.Run.Index))
			set(CIPassed, Succeeded, fmt.Sprintf("run #%d", run.Run.Index))
		} else {
			v.CI.Route = "already deployed"
			for _, m := range []Milestone{CIStarted, CheckHostsPassed, CIPassed} {
				set(m, Succeeded, detail)
			}
		}
		set(AutoUpdateAdvanced, Succeeded, detail)
		return v
	}

	// ci_started
	switch {
	case len(o.CheckRuns) > 0:
		r := o.CheckRuns[len(o.CheckRuns)-1]
		set(CIStarted, Succeeded, runLabel(r))
	case o.BumpRunning:
		set(CIStarted, Pending, "no check.yml run; the daily flake bump (update.yml) is running from a main that contains it")
		v.CI.Route = "update.yml"
	case !o.WatchedAhead:
		v.CI.Route = "none"
		when := "the next daily flake bump (update.yml, 09:00 UTC"
		if !o.NextBump.IsZero() {
			when += ", " + o.NextBump.UTC().Format("2006-01-02 15:04") + " UTC"
		}
		when += ")"
		reason := "it touches no path check.yml watches, and nothing after it on main does either, so no CI run will deploy it until another watched change lands or " + when
		// Ride the flake bump if the wait lasts long enough for it.
		if !o.NextBump.IsZero() && !o.Deadline.IsZero() && o.Deadline.After(o.NextBump.Add(90*time.Minute)) {
			set(CIStarted, Pending, "waiting for "+when+"; "+reason)
		} else {
			set(CIStarted, WillNotHappen, reason)
		}
	default:
		d := "waiting for a check.yml run to pick it up"
		if !o.PushedSince.IsZero() && o.At.Sub(o.PushedSince) > 10*time.Minute {
			d += fmt.Sprintf(" (none after %s; runner backlog?)", o.At.Sub(o.PushedSince).Round(time.Minute))
		}
		set(CIStarted, Pending, d)
	}
	if v.Stages[CIStarted].State != Succeeded {
		set(CheckHostsPassed, Pending, "")
		set(CIPassed, Pending, "")
		set(AutoUpdateAdvanced, Pending, "")
		return v
	}

	// check_hosts_passed: any containing run whose check-hosts succeeded wins;
	// otherwise one still running keeps us waiting; otherwise the latest failure.
	ch := evalJob(o.CheckRuns, "check-hosts")
	set(CheckHostsPassed, ch.state, ch.detail)
	if ch.state == Failed {
		v.CI = ciInfo(ch.run, "check.yml")
		if ch.run.CheckLog != nil {
			v.CI.FailedHosts = ch.run.CheckLog.Failed
			v.LogTail = checkLogTail(ch.run.CheckLog)
		} else {
			v.LogTail = ch.run.FailTail
		}
	}

	// ci_passed: the whole run.
	var passing, running *RunObs
	for i := range o.CheckRuns {
		r := &o.CheckRuns[i]
		switch {
		case r.Run.Status == "success":
			passing = r
		case !r.Run.Done():
			running = r
		}
	}
	switch {
	case passing != nil:
		v.CI = ciInfo(*passing, "check.yml")
		set(CIPassed, Succeeded, runLabel(*passing))
		if ch.state != Succeeded {
			set(CheckHostsPassed, Succeeded, runLabel(*passing))
		}
	case running != nil:
		d := runLabel(*running) + " running"
		if ch.state == Failed {
			// An earlier run failed but a later commit's run might fix it.
			d += fmt.Sprintf("; an earlier run (#%d) failed", ch.run.Run.Index)
			set(CheckHostsPassed, Pending, "a later run is still going: "+runLabel(*running))
		}
		set(CIPassed, Pending, d)
	default:
		last := o.CheckRuns[len(o.CheckRuns)-1]
		if v.CI.Run == 0 {
			v.CI = ciInfo(last, "check.yml")
		}
		reason := fmt.Sprintf("%s %s", runLabel(last), last.Run.Status)
		if last.FailJob != "" {
			reason += ": job " + last.FailJob + " failed"
		}
		if ch.state == Failed && len(v.CI.FailedHosts) > 0 {
			reason += " (check-hosts failed for " + strings.Join(v.CI.FailedHosts, ", ") + ")"
		}
		set(CIPassed, Failed, reason)
		if v.LogTail == "" {
			v.LogTail = last.FailTail
		}
	}

	// auto_update_advanced
	switch {
	case v.Stages[CIPassed].State != Succeeded:
		set(AutoUpdateAdvanced, Pending, "")
	case passing != nil && !passing.Run.Stopped.IsZero() && o.At.Sub(passing.Run.Stopped) > 3*time.Minute:
		set(AutoUpdateAdvanced, Failed, fmt.Sprintf(
			"CI passed in run #%d but auto-update (%s) does not contain it: the branch was moved off it",
			passing.Run.Index, short(o.AutoUpdateSHA)))
	default:
		set(AutoUpdateAdvanced, Pending, "CI passed; waiting for the branch to move")
	}
	return v
}

type jobVerdict struct {
	state  State
	detail string
	run    RunObs
}

func evalJob(runs []RunObs, name string) jobVerdict {
	var pending *RunObs
	var lastFail *RunObs
	for i := range runs {
		r := &runs[i]
		j, ok := r.job(name)
		if !ok {
			if r.Run.Status == "success" {
				return jobVerdict{Succeeded, runLabel(*r), *r}
			}
			if !r.Run.Done() {
				pending = r
			} else {
				lastFail = r
			}
			continue
		}
		switch {
		case j.Status == "success":
			return jobVerdict{Succeeded, runLabel(*r), *r}
		case !jobDone(j.Status):
			pending = r
		default:
			lastFail = r
		}
	}
	if pending != nil {
		return jobVerdict{Pending, runLabel(*pending) + ": " + name + " running", *pending}
	}
	if lastFail != nil {
		d := runLabel(*lastFail) + ": " + name + " failed"
		if lastFail.CheckLog != nil && len(lastFail.CheckLog.Failed) > 0 {
			d += " for " + strings.Join(lastFail.CheckLog.Failed, ", ")
		}
		return jobVerdict{Failed, d, *lastFail}
	}
	return jobVerdict{Pending, "", RunObs{}}
}

func latestSuccess(runs []RunObs) (RunObs, bool) {
	for i := len(runs) - 1; i >= 0; i-- {
		if runs[i].Run.Status == "success" {
			return runs[i], true
		}
	}
	return RunObs{}, false
}

func runLabel(r RunObs) string {
	s := fmt.Sprintf("run #%d", r.Run.Index)
	if !r.Own {
		s += " (for later commit " + short(r.Run.CommitSHA) + ")"
	}
	return s
}

func ciInfo(r RunObs, route string) CIInfo {
	return CIInfo{Run: r.Run.Index, RunURL: r.Run.HTMLURL, RunCommit: r.Run.CommitSHA, Superseded: !r.Own, Route: route}
}

func checkLogTail(l *forgejo.CheckLog) string {
	var b strings.Builder
	for _, h := range l.Hosts {
		if !h.OK && h.Tail != "" {
			fmt.Fprintf(&b, "== %s ==\n%s\n", h.Host, h.Tail)
		}
	}
	if b.Len() == 0 {
		return l.Tail
	}
	s := b.String()
	if len(s) > 8192 {
		s = "…" + s[len(s)-8192:]
	}
	return s
}

// NextBump is the next 09:00 UTC after t (update.yml's cron).
func NextBump(t time.Time) time.Time {
	u := t.UTC()
	n := time.Date(u.Year(), u.Month(), u.Day(), 9, 0, 0, 0, time.UTC)
	if !n.After(u) {
		n = n.Add(24 * time.Hour)
	}
	return n
}
