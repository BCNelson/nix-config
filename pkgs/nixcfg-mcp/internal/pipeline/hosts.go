package pipeline

import (
	"errors"
	"fmt"
	"time"

	"nixcfg-mcp/internal/config"
	"nixcfg-mcp/internal/hostprobe"
	"nixcfg-mcp/internal/humancmd"
)

// Tri is a three-valued ancestry answer (the commit may not be known locally).
type Tri int

const (
	Unknown Tri = iota
	No
	Yes
)

// HostObs is one observation of a host.
type HostObs struct {
	Host  config.Host
	State *hostprobe.State // nil when the probe failed
	Err   error

	// Does each of the host's commits contain sha?
	HeadContains    Tri
	BuiltContains   Tri
	AttemptContains Tri
	// The commit the host is failing on is the newest on auto-update, so
	// nothing will supersede it.
	AttemptIsTip bool

	Advanced    bool      // auto_update_advanced has succeeded
	AdvancedAt  time.Time // when it was first seen
	Now         time.Time
	Deadline    time.Time
	JournalTail string // fetched by the observer when a run failed
}

const (
	unreachableAfter       = 10 * time.Minute
	unreachableAfterReboot = 20 * time.Minute
	ntfyGrace              = 10 * time.Minute
)

// EvalHost advances one host's progress toward target and returns new events.
// p carries bookkeeping between observations and is updated in place.
func EvalHost(p *HostProgress, target Milestone, o HostObs) []Event {
	var evs []Event
	emit := func(stage Milestone, kind, text string) {
		evs = append(evs, Event{At: o.Now, Stage: stage, Host: p.Host, Kind: kind, Text: text})
	}
	reach := func(m Milestone) {
		if p.Stages == nil {
			p.Stages = map[Milestone]time.Time{}
		}
		if _, ok := p.Stages[m]; !ok {
			p.Stages[m] = o.Now
			if hostIndex(m) > hostIndex(p.Stage) || p.Stage == "" {
				p.Stage = m
			}
			emit(m, "ok", "reached")
		}
	}
	finish := func(s State, stage Milestone, detail string, human ...string) {
		p.State = s
		if stage != "" {
			p.Stage = stage // where it ended: the failing stage, or the one reached
		}
		p.Detail = detail
		p.Human = append(p.Human[:0], human...)
		kind := "fail"
		switch s {
		case Succeeded:
			kind = "ok"
		case Blocked:
			kind = "blocked"
		}
		emit(stage, kind, detail)
	}
	if p.State.Terminal() {
		return nil
	}
	h := o.Host

	if !h.AutoUpdating() {
		finish(WillNotHappen, HostFetched, "this host does not run auto-update ("+h.Note+")")
		return evs
	}
	if !h.Observable() {
		finish(WillNotHappen, HostFetched, "unobservable: no ssh destination in .agents/hosts.json, so nixcfg can't see whether it applied")
		return evs
	}

	// Probe failed.
	if o.State == nil {
		if errors.Is(o.Err, hostprobe.ErrNeedsLogin) {
			if p.LastError != "needs-login" {
				emit(p.Stage, "blocked", "Tailscale SSH wants a browser login before nixcfg can look; hand the human: "+humancmd.SSHLogin(h))
			}
			p.LastError = "needs-login"
			p.Human = []string{humancmd.SSHLogin(h)}
			if !o.Deadline.IsZero() && o.Now.After(o.Deadline) {
				finish(Blocked, p.Stage, "Tailscale SSH check mode: a browser login is needed", humancmd.SSHLogin(h))
			}
			return evs
		}
		var off *hostprobe.OfflineError
		if errors.As(o.Err, &off) && !p.RebootExpected && (off.LastSeen.Year() < 2000 || o.Now.Sub(off.LastSeen) > time.Hour) {
			// Long gone from the tailnet, not mid-reboot: waiting won't help.
			finish(Unreachable, p.Stage, off.Error())
			return evs
		}
		if p.FailStreakSince.IsZero() {
			p.FailStreakSince = o.Now
		}
		msg := fmt.Sprint(o.Err)
		if msg != p.LastError {
			emit(p.Stage, "warn", "probe failed: "+msg)
			p.LastError = msg
		}
		limit := unreachableAfter
		if p.RebootExpected {
			limit = unreachableAfterReboot
		}
		if o.Now.Sub(p.FailStreakSince) > limit {
			finish(Unreachable, p.Stage, fmt.Sprintf("unreachable for %s: %s", o.Now.Sub(p.FailStreakSince).Round(time.Second), msg))
		}
		return evs
	}
	if !p.FailStreakSince.IsZero() {
		emit(p.Stage, "info", fmt.Sprintf("reachable again after %s", o.Now.Sub(p.FailStreakSince).Round(time.Second)))
	}
	p.FailStreakSince = time.Time{}
	p.LastError = ""
	s := *o.State
	if p.BaseBootID == "" {
		p.BaseBootID = s.BootID
	}

	if !s.Enabled {
		finish(WillNotHappen, HostFetched, "auto-update.service is not installed on this host")
		return evs
	}

	applied := o.BuiltContains == Yes
	if applied {
		reach(HostFetched)
		reach(HostBuilding)
		if s.Branch != "auto-update" {
			emit(HostApplied, "warn", "applied, but the checkout at "+s.Checkout+" is on "+orDefault(s.Branch, "a detached HEAD")+", not auto-update: it deploys that branch without waiting for CI. Hand the human: "+humancmd.SwitchBranch(h, s.Checkout))
		}
	}

	// Wrong branch: auto-update.sh deploys whatever is checked out.
	if s.Branch == "main" && !applied {
		// main contains everything pushed, so the host still gets the
		// commit -- ungated, but it does. Warn once and keep following it.
		if len(p.Human) == 0 {
			emit(HostFetched, "warn", "the checkout at "+s.Checkout+" is on main, not auto-update: it deploys main without waiting for CI. Hand the human: "+humancmd.SwitchBranch(h, s.Checkout))
			p.Human = []string{humancmd.SwitchBranch(h, s.Checkout)}
		}
	} else if s.Branch != "auto-update" && !applied {
		if s.Branch == "" {
			finish(WillNotHappen, HostFetched, "the checkout at "+s.Checkout+" is on a detached HEAD; auto-update refuses to run", humancmd.SwitchBranch(h, s.Checkout))
		} else {
			finish(WillNotHappen, HostFetched, "the checkout at "+s.Checkout+" is on branch "+s.Branch+", not auto-update, so it deploys that branch instead", humancmd.SwitchBranch(h, s.Checkout))
		}
		return evs
	}

	if applied {
		if s.SwitchPending() {
			// Built and staged with `nixos-rebuild boot`; only a reboot activates it.
			p.RebootExpected = s.Reboot
			if !s.Reboot {
				finish(WillNotHappen, HostApplied, "built, but staged for the next boot (switch was inhibited) and this host doesn't reboot itself", humancmd.Reboot(h))
				return evs
			}
			if s.RebootScheduled {
				p.ETA = "rebooting within a minute"
			}
			p.Detail = "built and staged for the next boot; waiting for it to reboot"
			if s.BootID != p.BaseBootID {
				p.Detail = "rebooted, but the new system is still not active"
			}
			return evs
		}
		reach(HostApplied)
		p.ETA = ""
		if target == HostApplied || target == AllHostsApplied {
			finish(Succeeded, HostApplied, "applied ("+short(s.BuiltCommit)+")")
			return evs
		}
		// live / host_rebooted_ok
		if !s.NeedsReboot() {
			if s.BootID != p.BaseBootID {
				if s.SystemState != "running" && s.SystemState != "degraded" {
					p.Detail = "rebooted; waiting for boot to finish (" + s.SystemState + ")"
					return evs
				}
				reach(HostRebootedOK)
				finish(Succeeded, HostRebootedOK, "applied and rebooted into it ("+short(s.BuiltCommit)+", "+s.SystemState+")")
				return evs
			}
			reach(HostRebootedOK)
			finish(Succeeded, HostRebootedOK, "applied ("+short(s.BuiltCommit)+"); no reboot needed")
			return evs
		}
		p.RebootExpected = s.Reboot
		if !s.Reboot {
			finish(WillNotHappen, HostRebootedOK, "applied, but the kernel/initrd changed and this host doesn't reboot itself", humancmd.Reboot(h))
			return evs
		}
		if s.RebootScheduled {
			p.ETA = "rebooting within a minute"
			p.Detail = "applied; reboot scheduled"
		} else {
			p.Detail = "applied; a reboot is needed and the host should schedule it"
			if at, ok := p.Stages[HostApplied]; ok && o.Now.Sub(at) > 15*time.Minute && !s.Running() {
				finish(Failed, HostRebootedOK, "applied 15m ago and a reboot is needed, but none is scheduled", humancmd.Reboot(h))
			}
		}
		return evs
	}

	// Not applied yet.
	if o.HeadContains == Yes || o.AttemptContains == Yes {
		reach(HostFetched)
	}
	if o.AttemptContains == Yes {
		reach(HostBuilding) // it has started (and maybe failed) a build of it
	}
	if s.Running() {
		if o.AttemptContains == Yes {
			p.Detail = fmt.Sprintf("building, attempt %d/%d", s.AttemptCount, s.MaxRetries)
		} else {
			p.Detail = "auto-update running (fetching)"
		}
		p.ETA = ""
		return evs
	}

	// Idle and not applied.
	why := "waiting for auto-update to start"
	if o.AttemptContains == Yes && s.AttemptCount >= s.MaxRetries {
		if o.AttemptIsTip {
			finish(Failed, HostBuilding,
				fmt.Sprintf("build failed %d/%d times for %s; the host won't retry until a new commit reaches auto-update", s.AttemptCount, s.MaxRetries, short(s.AttemptCommit)),
				humancmd.ResetRetries(h))
			p.LogTail = o.JournalTail
			return evs
		}
		why = fmt.Sprintf("retries exhausted for %s; a newer commit is on auto-update, waiting for the host to pick it up", short(s.AttemptCommit))
	} else if o.AttemptContains == Yes && s.Result != "success" && s.Invocation != p.SeenInvocation {
		p.SeenInvocation = s.Invocation
		emit(HostBuilding, "warn", fmt.Sprintf("attempt %d/%d failed (%s); it retries on the next run", s.AttemptCount, s.MaxRetries, s.Result))
		why = fmt.Sprintf("attempt %d/%d failed; waiting for the next run", s.AttemptCount, s.MaxRetries)
		if o.JournalTail != "" {
			p.LogTail = o.JournalTail
		}
	} else if s.Result != "" && s.Result != "success" && o.AttemptContains != Yes && o.Advanced &&
		s.Invocation != p.SeenInvocation && s.ExitedAt.After(o.AdvancedAt) {
		// It ran after the branch moved, failed, and never got as far as
		// building: fetch, rebase, git-crypt, ...
		p.SeenInvocation = s.Invocation
		p.FailedRuns = append(p.FailedRuns, s.Invocation)
		p.LogTail = o.JournalTail
		emit(HostFetched, "warn", "auto-update failed before building ("+s.Result+")")
		if len(p.FailedRuns) >= 2 {
			finish(Failed, HostFetched, fmt.Sprintf("auto-update failed %d times before building (see log tail)", len(p.FailedRuns)))
			return evs
		}
	}
	// auto-update.sh refuses to run while git-crypt is locked. Only call it
	// terminal once a run has actually failed: the probe's read of the repo
	// config could be wrong for reasons of its own.
	if !s.Unlocked && s.Result != "" && s.Result != "success" {
		finish(Failed, HostFetched, "git-crypt is locked in the host checkout, so auto-update refuses to run")
		return evs
	}

	// Waiting for a trigger.
	if !o.Advanced {
		p.Detail = "waiting for the auto-update branch to advance"
		return evs
	}
	p.Detail = why
	next := s.NextTimer
	switch {
	case s.Ntfy && o.Now.Sub(o.AdvancedAt) < ntfyGrace:
		p.ETA = "ntfy should start it within a couple of minutes"
	case !next.IsZero():
		p.ETA = "timer fires at " + next.Local().Format("15:04") + " (in " + next.Sub(o.Now).Round(time.Minute).String() + ")"
		if !o.Deadline.IsZero() && next.After(o.Deadline.Add(-5*time.Minute)) {
			why := "no ntfy subscriber"
			if s.Ntfy {
				why = "the ntfy ping didn't start it"
			}
			finish(Blocked, HostFetched, fmt.Sprintf("%s, and the next timer run (%s) is after the wait's deadline", why, next.Local().Format("15:04")), humancmd.Kick(h))
			return evs
		}
	default:
		p.ETA = "unknown (no timer information)"
	}
	return evs
}
