package pipeline

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"math/big"
	"sort"
	"strings"
	"sync"
	"time"

	"nixcfg-mcp/internal/config"
	"nixcfg-mcp/internal/hostprobe"
	"nixcfg-mcp/internal/humancmd"
)

// book is bookkeeping that is not part of the reported result.
type book struct {
	PushedSince   time.Time `json:"pushed_since,omitempty"`
	AdvancedAt    time.Time `json:"advanced_at,omitempty"`
	LastHostProbe time.Time `json:"last_host_probe,omitempty"`
	LastObsError  string    `json:"last_obs_error,omitempty"`
	ObsErrorSince time.Time `json:"obs_error_since,omitempty"`
}

// Wait is one running (or finished) wait.
type Wait struct {
	mu       sync.Mutex
	res      Result
	book     book
	events   []Event
	store    *Store
	obs      *Observer
	cfg      *config.Config
	notify   chan struct{} // closed and replaced on every change
	done     chan struct{}
	doneOnce sync.Once
	// lastDetail dedupes progress events per stage/host.
	lastDetail map[string]string
}

// NewWait validates a spec and selects hosts.
func NewWait(cfg *config.Config, obs *Observer, store *Store, spec Spec) (*Wait, error) {
	if spec.Timeout <= 0 {
		spec.Timeout = 60 * time.Minute
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	full, err := obs.Git.RevParse(ctx, spec.SHA)
	if err != nil || full == "" {
		// Maybe it was pushed from elsewhere: fetch and retry.
		_, _ = obs.Git.Fetch(ctx, "main", "auto-update")
		full, err = obs.Git.RevParse(ctx, spec.SHA)
		if err != nil || full == "" {
			return nil, fmt.Errorf("unknown commit %q (not in this checkout or on origin)", spec.SHA)
		}
	}
	spec.SHA = full
	now := time.Now()
	w := &Wait{
		store: store, obs: obs, cfg: cfg,
		notify:     make(chan struct{}),
		done:       make(chan struct{}),
		lastDetail: map[string]string{},
	}
	w.res = Result{
		ID:       newID(now),
		Spec:     spec,
		Subject:  obs.Git.Subject(ctx, full),
		State:    Pending,
		Created:  now,
		Deadline: now.Add(spec.Timeout),
		Updated:  now,
	}
	for _, m := range RepoStages {
		w.res.Stages = append(w.res.Stages, Stage{Name: m, State: Pending})
	}
	if spec.Until.IsHostStage() {
		sel, err := SelectHosts(cfg, spec.Hosts, hostprobe.Tailnet(ctx))
		if err != nil {
			return nil, err
		}
		w.res.HostSelection = sel
		w.res.Hosts = map[string]*HostProgress{}
		for _, h := range sel.Hosts {
			w.res.Hosts[h] = &HostProgress{Host: h, State: Pending}
		}
		if len(sel.Hosts) == 0 {
			return nil, fmt.Errorf("no hosts to wait for (excluded: %s); name some with --host", describeExcluded(sel.Excluded))
		}
	}
	w.res.CurrentStage = Pushed
	w.addEvent(Event{At: now, Stage: Pushed, Kind: "info", Text: fmt.Sprintf("waiting for %s (%s) to reach %s; deadline %s", short(full), w.res.Subject, spec.Until, w.res.Deadline.Local().Format("15:04"))})
	if sel := w.res.HostSelection; sel != nil {
		text := "hosts: " + strings.Join(sel.Hosts, ", ")
		if !sel.Explicit && len(sel.Excluded) > 0 {
			text += "; left out: " + describeExcluded(sel.Excluded)
		}
		w.addEvent(Event{At: now, Stage: HostFetched, Kind: "info", Text: text})
	}
	return w, nil
}

func describeExcluded(ex map[string]string) string {
	names := make([]string, 0, len(ex))
	for n := range ex {
		names = append(names, n)
	}
	sort.Strings(names)
	parts := make([]string, len(names))
	for i, n := range names {
		parts[i] = n + " (" + ex[n] + ")"
	}
	return strings.Join(parts, ", ")
}

// SelectHosts picks the hosts a host-stage wait covers.
// peers is the tailnet's view (nil when unknown); a default selection leaves
// out hosts it says are offline, since they can only end unreachable.
func SelectHosts(cfg *config.Config, explicit []string, peers map[string]hostprobe.Peer) (*HostSelection, error) {
	sel := &HostSelection{Excluded: map[string]string{}}
	if len(explicit) > 0 {
		sel.Explicit = true
		for _, n := range explicit {
			h, err := cfg.Host(n)
			if err != nil {
				return nil, err
			}
			sel.Hosts = append(sel.Hosts, h.Name)
		}
		return sel, nil
	}
	for _, n := range cfg.HostNames() {
		h := cfg.Hosts[n]
		iv := h.IntervalDuration()
		switch {
		case !h.AutoUpdating():
			sel.Excluded[n] = "not auto-updating"
		case !h.Observable():
			sel.Excluded[n] = "unobservable"
		case !h.Ntfy && (iv == 0 || iv > 15*time.Minute):
			sel.Excluded[n] = "no ntfy; timer every " + h.Interval
		case peers != nil && !peers[n].Online && peers[n].HostName != "":
			sel.Excluded[n] = "offline on the tailnet"
			if ls := peers[n].LastSeen; ls.Year() >= 2000 {
				sel.Excluded[n] += ", last seen " + hostprobe.Ago(time.Since(ls)) + " ago"
			}
		default:
			sel.Hosts = append(sel.Hosts, n)
		}
	}
	return sel, nil
}

func newID(t time.Time) string {
	b := make([]byte, 3)
	_, _ = rand.Read(b)
	return "w-" + t.UTC().Format("20060102-150405") + "-" + hex.EncodeToString(b)
}

// ID returns the wait id.
func (w *Wait) ID() string { return w.res.ID }

// Snapshot returns the result and the events from cursor on.
func (w *Wait) Snapshot(cursor int) (Result, []Event) {
	w.mu.Lock()
	defer w.mu.Unlock()
	r := w.res
	r.Hosts = map[string]*HostProgress{}
	for k, v := range w.res.Hosts {
		c := *v
		r.Hosts[k] = &c
	}
	r.Stages = append([]Stage(nil), w.res.Stages...)
	if cursor < 0 || cursor > len(w.events) {
		cursor = 0
	}
	return r, append([]Event(nil), w.events[cursor:]...)
}

// Changed returns a channel closed at the next change.
func (w *Wait) Changed() <-chan struct{} {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.notify
}

// Done is closed when the wait reaches a terminal state.
func (w *Wait) Done() <-chan struct{} { return w.done }

func (w *Wait) addEvent(e Event) {
	// Host checks re-derive their warnings on every poll (a checkout on the
	// wrong branch stays wrong), so the same warning would otherwise repeat
	// for the whole wait. Scanning is fine: a wait has a few dozen events, and
	// it also dedupes across a resume because the old events are loaded.
	if e.Kind == "warn" {
		for _, old := range w.events {
			if old.Kind == "warn" && old.Host == e.Host && old.Text == e.Text {
				return
			}
		}
	}
	w.events = append(w.events, e)
	w.res.EventCount = len(w.events)
	if w.store != nil {
		w.store.appendEvent(w.res.ID, e)
	}
	close(w.notify)
	w.notify = make(chan struct{})
}

// progress emits a progress event when a stage's (or host's) detail changes.
func (w *Wait) progress(stage Milestone, host, kind, text string) {
	key := string(stage) + "/" + host
	if text == "" || w.lastDetail[key] == text {
		return
	}
	w.lastDetail[key] = text
	w.addEvent(Event{At: time.Now(), Stage: stage, Host: host, Kind: kind, Text: text})
}

// Run polls until the wait is terminal or ctx ends.
func (w *Wait) Run(ctx context.Context) {
	defer func() {
		w.persist()
		w.mu.Lock()
		terminal := w.res.State.Terminal()
		w.mu.Unlock()
		if terminal {
			w.closeDone()
		}
	}()
	for {
		w.step(ctx)
		w.persist()
		if w.res.State.Terminal() {
			return
		}
		select {
		case <-ctx.Done():
			return
		case <-time.After(w.interval()):
		}
	}
}

func (w *Wait) interval() time.Duration {
	w.mu.Lock()
	defer w.mu.Unlock()
	// 15s for host stages too: the status line shows each poll's result, and
	// the probes are a cheap ssh each.
	base := 15 * time.Second
	if w.res.CurrentStage == CIStarted || w.res.CurrentStage == CheckHostsPassed {
		base = 20 * time.Second
	}
	if w.book.LastObsError != "" {
		// Back off while the network is unhappy, up to 2 min.
		since := time.Since(w.book.ObsErrorSince)
		base = min(2*time.Minute, max(base, since/2))
	}
	j, _ := rand.Int(rand.Reader, big.NewInt(int64(base/5)))
	return base + time.Duration(j.Int64())
}

func (w *Wait) step(ctx context.Context) {
	now := time.Now()
	sha := w.res.Spec.SHA

	refs, err := w.obs.Refs(ctx)
	if err != nil {
		w.mu.Lock()
		w.obsError(now, err.Error())
		w.checkDeadline(now)
		w.mu.Unlock()
		return
	}
	w.mu.Lock()
	pushedSince := w.book.PushedSince
	deadline := w.res.Deadline
	w.mu.Unlock()

	ro := w.obs.Repo(ctx, sha, refs, pushedSince, deadline)
	v := EvalRepo(ro)

	w.mu.Lock()
	if len(ro.Errors) > 0 {
		w.obsError(now, strings.Join(ro.Errors, "; "))
	} else {
		w.book.LastObsError = ""
	}
	if ro.Pushed && w.book.PushedSince.IsZero() {
		w.book.PushedSince = now
	}
	advanced := v.Stages[AutoUpdateAdvanced].State == Succeeded
	if advanced && w.book.AdvancedAt.IsZero() {
		w.book.AdvancedAt = now
	}
	if v.CI.Run != 0 || v.CI.Route != "" {
		ci := v.CI
		w.res.CI = &ci
	}
	terminal := w.mergeRepo(now, v)
	w.res.Updated = now
	// Save before probing hosts (which can take a while), so a process that
	// dies mid-probe doesn't make its successor re-announce these stages.
	if w.store != nil {
		w.store.save(&w.res, w.book)
	}
	if terminal {
		w.mu.Unlock()
		return
	}
	until := w.res.Spec.Until
	if !until.IsHostStage() {
		if st := w.stage(until); st.State == Succeeded {
			w.finish(now, Succeeded, until, "", st.Detail, "", nil)
		}
		w.checkDeadline(now)
		w.mu.Unlock()
		return
	}
	// Probe hosts every 30s once the branch moved, every 90s before (to fail
	// fast on wrong branches, disabled or unreachable hosts).
	probeEvery := 90 * time.Second
	if advanced {
		probeEvery = 25 * time.Second
	}
	if now.Sub(w.book.LastHostProbe) < probeEvery {
		w.checkDeadline(now)
		w.mu.Unlock()
		return
	}
	w.book.LastHostProbe = now
	advancedAt := w.book.AdvancedAt
	var todo []*HostProgress
	for _, n := range w.res.HostSelection.Hosts {
		if p := w.res.Hosts[n]; !p.State.Terminal() {
			todo = append(todo, p)
		}
	}
	w.mu.Unlock()

	// Probe hosts in parallel, outside the lock.
	type res struct {
		p   *HostProgress
		obs HostObs
	}
	results := make([]res, len(todo))
	var wg sync.WaitGroup
	for i, p := range todo {
		wg.Add(1)
		go func(i int, p *HostProgress) {
			defer wg.Done()
			h := w.cfg.Hosts[p.Host]
			o := w.obs.Host(ctx, h, sha, refs)
			o.Advanced = advanced
			o.AdvancedAt = advancedAt
			o.Deadline = deadline
			if o.State != nil && o.State.Result != "" && o.State.Result != "success" &&
				o.State.Invocation != "" && o.State.Invocation != p.SeenInvocation && !o.State.Running() {
				o.JournalTail = w.obs.Journal(ctx, h, o.State.Invocation)
			}
			results[i] = res{p, o}
		}(i, p)
	}
	wg.Wait()

	w.mu.Lock()
	defer w.mu.Unlock()
	target := until.HostTarget()
	for _, r := range results {
		for _, e := range EvalHost(r.p, target, r.obs) {
			w.addEvent(e)
		}
		if !r.p.State.Terminal() {
			w.progress(r.p.Stage, r.p.Host, "progress", joinNonEmpty(r.p.Detail, stableETA(r.p.ETA)))
		}
	}
	w.aggregateHosts(now)
	if !w.res.State.Terminal() {
		w.checkDeadline(now)
	}
}

// stableETA drops a trailing "(in 48m0s)" countdown. progress() only emits
// when the text changes, and a countdown changes every poll: a host waiting
// an hour for its timer would print a line a minute. The absolute time before
// it says the same thing; the rendered summary still shows the countdown.
func stableETA(eta string) string {
	if i := strings.LastIndex(eta, " (in "); i >= 0 && strings.HasSuffix(eta, ")") {
		return eta[:i]
	}
	return eta
}

func joinNonEmpty(parts ...string) string {
	var out []string
	for _, p := range parts {
		if p != "" {
			out = append(out, p)
		}
	}
	return strings.Join(out, "; ")
}

func (w *Wait) obsError(now time.Time, msg string) {
	if w.book.LastObsError == "" {
		w.book.ObsErrorSince = now
	}
	if w.book.LastObsError != msg {
		w.addEvent(Event{At: now, Stage: w.res.CurrentStage, Kind: "warn", Text: "observation failed (will retry): " + msg})
	}
	w.book.LastObsError = msg
}

func (w *Wait) stage(m Milestone) Stage {
	for _, s := range w.res.Stages {
		if s.Name == m {
			return s
		}
	}
	return Stage{}
}

// mergeRepo folds a repo verdict into the timeline; it reports whether the
// wait became terminal.
func (w *Wait) mergeRepo(now time.Time, v RepoVerdict) bool {
	until := w.res.Spec.Until
	lastRepo := AutoUpdateAdvanced
	if !until.IsHostStage() {
		lastRepo = until
	}
	current := Milestone("")
	for i := range w.res.Stages {
		st := &w.res.Stages[i]
		nv, ok := v.Stages[st.Name]
		if !ok {
			continue
		}
		if st.State == Succeeded && nv.State != Succeeded {
			// A stage can't un-succeed (e.g. a later observation lost CI
			// info); keep it.
			continue
		}
		if nv.State != Pending && st.StartedAt.IsZero() {
			st.StartedAt = now
		}
		if nv.State == Pending && st.StartedAt.IsZero() && nv.Detail != "" {
			st.StartedAt = now
		}
		changed := nv.State != st.State
		st.State = nv.State
		st.Detail = nv.Detail
		if changed && nv.State.Terminal() {
			st.FinishedAt = now
			kind := "ok"
			if nv.State != Succeeded {
				kind = "fail"
			}
			w.addEvent(Event{At: now, Stage: st.Name, Kind: kind, Text: orDefault(nv.Detail, string(nv.State))})
		} else if nv.State == Pending {
			w.progress(st.Name, "", "progress", nv.Detail)
		}
		if current == "" && st.State != Succeeded {
			current = st.Name
		}
		if repoIndex(st.Name) > repoIndex(lastRepo) {
			continue
		}
		if nv.State.Terminal() && nv.State != Succeeded {
			w.finish(now, nv.State, st.Name, "", nv.Detail, v.LogTail, v.Human)
			return true
		}
	}
	if len(v.Human) > 0 {
		w.res.HumanActions = v.Human
		for _, h := range v.Human {
			w.progress(Pushed, "", "blocked", "needs the human: "+h)
		}
	}
	if current == "" {
		current = HostFetched
		if !until.IsHostStage() {
			current = until
		}
	}
	w.res.CurrentStage = current
	return false
}

func orDefault(s, d string) string {
	if s == "" {
		return d
	}
	return s
}

// aggregateHosts ends the wait once the hosts decide it. A host that fails
// for a reason about the change (build failed, wrong branch, ...) ends it at
// once unless keep_going. A host that is merely out of reach (unreachable,
// timed out) never does, unless strict: the wait carries on with the others
// and ends partial if they all made it.
func (w *Wait) aggregateHosts(now time.Time) {
	sel := w.res.HostSelection
	strict := w.res.Spec.Strict
	all := true
	var hard, soft []*HostProgress
	ok := 0
	minStage := HostRebootedOK
	for _, n := range sel.Hosts {
		p := w.res.Hosts[n]
		switch {
		case !p.State.Terminal():
			all = false
			if hostIndex(p.Stage) < hostIndex(minStage) {
				minStage = p.Stage
			}
		case p.State == Succeeded:
			ok++
		case p.State.Soft() && !strict:
			soft = append(soft, p)
		default:
			hard = append(hard, p)
		}
	}
	if minStage == "" {
		minStage = HostFetched
	}
	if w.stage(AutoUpdateAdvanced).State == Succeeded {
		w.res.CurrentStage = minStage
	}
	if len(hard) > 0 && (!w.res.Spec.KeepGoing || all) {
		worst := hard[0]
		reason := worst.Detail
		if len(sel.Hosts) > 1 {
			reason = worst.Host + ": " + reason
		}
		w.finish(now, worst.State, worst.Stage, worst.Host, reason, worst.LogTail, worst.Human)
		return
	}
	if !all {
		return
	}
	switch {
	case len(soft) == 0:
		w.finish(now, Succeeded, w.res.Spec.Until, "", fmt.Sprintf("%d host(s) reached %s", len(sel.Hosts), w.res.Spec.Until), "", nil)
	case ok == 0:
		// Nobody could be reached: that is not a partial success.
		w.finish(now, soft[0].State, soft[0].Stage, soft[0].Host, describeSoft(soft), "", nil)
	default:
		w.finish(now, Partial, w.res.Spec.Until, "",
			fmt.Sprintf("%d host(s) reached %s; %s", ok, w.res.Spec.Until, describeSoft(soft)), "", nil)
	}
}

func describeSoft(ps []*HostProgress) string {
	parts := make([]string, len(ps))
	for i, p := range ps {
		parts[i] = fmt.Sprintf("%s %s (%s)", p.Host, p.State, p.Detail)
	}
	return "didn't finish: " + strings.Join(parts, "; ")
}

func (w *Wait) checkDeadline(now time.Time) {
	if w.res.State.Terminal() || now.Before(w.res.Deadline) {
		return
	}
	if w.stage(Pushed).State != Succeeded {
		w.finish(now, TimedOut, Pushed, "", "never reached origin/main before the deadline (git push origin main)", "", nil)
		return
	}
	// Out of time on the host stages: each host still pending times out on
	// its own, and the usual rules decide the whole (partial when the others
	// made it).
	if w.res.HostSelection != nil && w.stage(AutoUpdateAdvanced).State == Succeeded {
		for _, n := range sortedKeys(w.res.Hosts) {
			p := w.res.Hosts[n]
			if p.State.Terminal() {
				continue
			}
			why := joinNonEmpty(p.Detail, p.ETA)
			switch {
			case p.LastError == "needs-login":
				p.State, p.Detail = Blocked, "Tailscale SSH check mode needs a browser login"
			case p.LastError != "" && !p.FailStreakSince.IsZero():
				p.State = Unreachable
				p.Detail = fmt.Sprintf("unreachable for %s: %s", now.Sub(p.FailStreakSince).Round(time.Second), p.LastError)
			default:
				p.State = TimedOut
				p.Detail = "deadline reached at " + orDefault(string(p.Stage), "start") + " (" + why + ")"
				p.Human = append(p.Human, humancmd.Journal(w.cfg.Hosts[n]))
			}
			w.addEvent(Event{At: now, Stage: p.Stage, Host: n, Kind: "fail", Text: p.Detail})
		}
		w.aggregateHosts(now)
		return
	}
	var stuck []string
	var human []string
	for _, n := range sortedKeys(w.res.Hosts) {
		p := w.res.Hosts[n]
		if !p.State.Terminal() {
			why := joinNonEmpty(p.Detail, p.ETA)
			if p.LastError != "" && !p.FailStreakSince.IsZero() {
				why = fmt.Sprintf("unreachable for %s: %s", now.Sub(p.FailStreakSince).Round(time.Second), p.LastError)
			}
			stuck = append(stuck, fmt.Sprintf("%s at %s (%s)", n, orDefault(string(p.Stage), "start"), why))
			human = append(human, p.Human...)
			if p.LastError == "needs-login" {
				w.finish(now, Blocked, p.Stage, n, "Tailscale SSH check mode needs a browser login", "", p.Human)
				return
			}
		}
	}
	reason := fmt.Sprintf("deadline reached while at %s: %s", w.res.CurrentStage, w.stage(w.res.CurrentStage).Detail)
	if len(stuck) > 0 {
		reason = "deadline reached; still waiting on " + strings.Join(stuck, "; ")
		for _, n := range sortedKeys(w.res.Hosts) {
			if !w.res.Hosts[n].State.Terminal() {
				human = append(human, humancmd.Journal(w.cfg.Hosts[n]))
				break
			}
		}
	}
	w.finish(now, TimedOut, w.res.CurrentStage, "", reason, "", human)
}

func sortedKeys(m map[string]*HostProgress) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

func (w *Wait) finish(now time.Time, s State, stage Milestone, host, reason, tail string, human []string) {
	if w.res.State.Terminal() {
		return
	}
	w.res.State = s
	if s != Succeeded && s != Partial {
		w.res.FailedStage = stage
		w.res.FailedHost = host
	}
	w.res.CurrentStage = stage
	w.res.Reason = reason
	w.res.LogTail = tail
	if len(human) > 0 {
		w.res.HumanActions = human
	}
	w.res.Finished = now
	w.res.Updated = now
	w.addEvent(Event{At: now, Stage: stage, Host: host, Kind: "result", Text: w.res.Summary()})
}

// Cancel stops a wait as timed_out("cancelled").
func (w *Wait) Cancel() {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.finish(time.Now(), TimedOut, w.res.CurrentStage, "", "cancelled", "", nil)
	w.closeDone()
}

func (w *Wait) closeDone() { w.doneOnce.Do(func() { close(w.done) }) }

func (w *Wait) persist() {
	if w.store == nil {
		return
	}
	w.mu.Lock()
	defer w.mu.Unlock()
	w.store.save(&w.res, w.book)
}

// Step runs one observation without the polling loop (deploy_status).
func (w *Wait) Step(ctx context.Context) { w.step(ctx) }
