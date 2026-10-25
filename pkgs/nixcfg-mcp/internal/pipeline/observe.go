package pipeline

import (
	"context"
	"fmt"
	"strconv"
	"strings"
	"sync"
	"time"

	"nixcfg-mcp/internal/config"
	"nixcfg-mcp/internal/forgejo"
	"nixcfg-mcp/internal/gitx"
	"nixcfg-mcp/internal/hostprobe"
	"nixcfg-mcp/internal/watch"
)

// Observer gathers the facts the evaluators judge. It is shared by every wait
// in a process and caches what can't change (finished runs, parsed logs,
// ancestry between known commits).
type Observer struct {
	Cfg    *config.Config
	Git    gitx.Repo
	FJ     *forgejo.Client
	Filter *watch.Filter

	mu        sync.Mutex
	anc       map[[2]string]bool
	runCache  map[int64]RunObs // finished runs
	filesSeen map[string]bool  // commit -> touches a watched path
	lastFetch time.Time
}

// NewObserver wires an observer for a repository.
func NewObserver(cfg *config.Config) (*Observer, error) {
	f, err := watch.Load(cfg.Root, cfg.Forgejo.Workflow)
	if err != nil {
		return nil, err
	}
	return &Observer{
		Cfg:       cfg,
		Git:       gitx.Repo{Dir: cfg.Root},
		FJ:        forgejo.New(cfg.Forgejo.URL, cfg.Forgejo.Repo),
		Filter:    f,
		anc:       map[[2]string]bool{},
		runCache:  map[int64]RunObs{},
		filesSeen: map[string]bool{},
	}, nil
}

// ensure makes sure commits are present locally, fetching main and
// auto-update at most every 20s.
func (o *Observer) ensure(ctx context.Context, shas ...string) {
	for _, s := range shas {
		if s == "" || o.Git.Has(ctx, s) {
			continue
		}
		o.mu.Lock()
		recent := time.Since(o.lastFetch) < 20*time.Second
		if !recent {
			o.lastFetch = time.Now()
		}
		o.mu.Unlock()
		if !recent {
			_, _ = o.Git.Fetch(ctx, "main", "auto-update")
		}
		return
	}
}

// Contains reports whether commit a is in the history of b.
func (o *Observer) Contains(ctx context.Context, a, b string) Tri {
	if a == "" || b == "" {
		return Unknown
	}
	if a == b {
		return Yes
	}
	key := [2]string{a, b}
	o.mu.Lock()
	v, ok := o.anc[key]
	o.mu.Unlock()
	if ok {
		return tri(v)
	}
	if !o.Git.Has(ctx, a) || !o.Git.Has(ctx, b) {
		o.ensure(ctx, a, b)
		if !o.Git.Has(ctx, a) || !o.Git.Has(ctx, b) {
			return Unknown
		}
	}
	yes, err := o.Git.IsAncestor(ctx, a, b)
	if err != nil {
		return Unknown
	}
	o.mu.Lock()
	o.anc[key] = yes
	o.mu.Unlock()
	return tri(yes)
}

func tri(b bool) Tri {
	if b {
		return Yes
	}
	return No
}

// Refs is the current main and auto-update.
type Refs struct {
	Main, AutoUpdate string
}

// Refs reads both branch tips from the server (falling back to the mirror).
func (o *Observer) Refs(ctx context.Context) (Refs, error) {
	m, _, err := o.Git.LsRemote(ctx, "main")
	if err != nil {
		return Refs{}, fmt.Errorf("reading main: %w", err)
	}
	a, _, err := o.Git.LsRemote(ctx, "auto-update")
	if err != nil {
		return Refs{}, fmt.Errorf("reading auto-update: %w", err)
	}
	o.ensure(ctx, m, a)
	return Refs{Main: m, AutoUpdate: a}, nil
}

// Repo observes the repo stages for sha.
func (o *Observer) Repo(ctx context.Context, sha string, refs Refs, pushedSince, deadline time.Time) RepoObs {
	now := time.Now()
	obs := RepoObs{At: now, MainSHA: refs.Main, AutoUpdateSHA: refs.AutoUpdate, PushedSince: pushedSince, Deadline: deadline, NextBump: NextBump(now)}
	obs.Pushed = o.Contains(ctx, sha, refs.Main) == Yes
	obs.InAutoUpdate = o.Contains(ctx, sha, refs.AutoUpdate) == Yes
	if !obs.Pushed {
		return obs
	}

	// What does sha (and what follows it on main) touch?
	if files, err := o.Git.CommitFiles(ctx, sha); err == nil {
		w, _ := o.Filter.Classify(files)
		for _, m := range w {
			obs.OwnWatched = append(obs.OwnWatched, m.Path)
		}
	}
	obs.WatchedAhead = o.watchedAhead(ctx, sha, refs.Main, refs.AutoUpdate)

	if obs.InAutoUpdate && pushedSince.IsZero() {
		// Long since deployed; skip the CI lookups.
		return obs
	}

	runs, err := o.FJ.RecentRuns(ctx)
	if err != nil {
		obs.Errors = append(obs.Errors, err.Error())
		return obs
	}
	obs.TypicalCI = TypicalDuration(runs, o.Cfg.Forgejo.Workflow)
	since := o.commitTime(ctx, sha).Add(-10 * time.Minute)
	for i := len(runs) - 1; i >= 0; i-- { // API returns newest first; walk oldest first
		r := runs[i]
		// Queued runs report started as the Unix epoch, not as unset.
		if r.Started.Unix() > 0 && r.Started.Before(since) {
			continue
		}
		if o.Contains(ctx, sha, r.CommitSHA) != Yes {
			continue
		}
		switch {
		case r.WorkflowID == o.Cfg.Forgejo.Workflow && (r.Event == "push" || r.Event == "workflow_dispatch") && r.PrettyRef == "main":
			obs.CheckRuns = append(obs.CheckRuns, o.runObs(ctx, r, sha))
		case r.WorkflowID == "update.yml" && !r.Done():
			obs.BumpRunning = true
		}
	}
	return obs
}

func (o *Observer) commitTime(ctx context.Context, sha string) time.Time {
	t, err := gitCommitTime(ctx, o.Git, sha)
	if err != nil {
		return time.Time{}
	}
	return t
}

// watchedAhead reports whether a CI run will (or did) carry sha: some commit
// that isn't deployed yet -- anything on main past auto-update, not just sha
// and its descendants -- touches a watched path. A push's path filter sees the
// whole pushed range, so an unwatched commit pushed together with a watched
// one still gets a run.
func (o *Observer) watchedAhead(ctx context.Context, sha, main, autoUpdate string) bool {
	var commits []string
	var err error
	if autoUpdate != "" {
		commits, err = o.Git.Commits(ctx, autoUpdate, main)
	}
	if autoUpdate == "" || err != nil {
		commits, err = o.Git.Commits(ctx, sha+"^", main)
	}
	if err != nil {
		commits = []string{sha}
	}
	for _, c := range commits {
		o.mu.Lock()
		v, ok := o.filesSeen[c]
		o.mu.Unlock()
		if !ok {
			files, err := o.Git.CommitFiles(ctx, c)
			if err != nil {
				return true // can't tell; don't claim it will never deploy
			}
			w, _ := o.Filter.Classify(files)
			v = len(w) > 0
			o.mu.Lock()
			o.filesSeen[c] = v
			o.mu.Unlock()
		}
		if v {
			return true
		}
	}
	return false
}

func (o *Observer) runObs(ctx context.Context, r forgejo.Run, sha string) RunObs {
	o.mu.Lock()
	cached, ok := o.runCache[r.ID]
	o.mu.Unlock()
	if ok && cached.Run.Status == r.Status {
		return cached
	}
	ro := RunObs{Run: r, Own: strings.HasPrefix(r.CommitSHA, sha) || strings.HasPrefix(sha, r.CommitSHA)}
	jobs, err := o.FJ.Jobs(ctx, r.ID)
	if err == nil {
		ro.Jobs = jobs
	}
	if r.Done() && r.Status != "success" {
		for idx, j := range ro.Jobs {
			if j.Status != "failure" && j.Status != "cancelled" {
				continue
			}
			ro.FailJob = j.Name
			if j.Name == "check-hosts" {
				p := forgejo.NewCheckLogParser()
				if err := o.FJ.Log(ctx, r.Index, idx, j.Attempt, p.Line); err == nil {
					l := p.Result()
					ro.CheckLog = &l
					ro.FailTail = l.Tail
				}
			} else {
				t := forgejo.NewTail(60)
				if err := o.FJ.Log(ctx, r.Index, idx, j.Attempt, t.Line); err == nil {
					ro.FailTail = t.String()
				}
			}
			break
		}
	}
	if r.Done() {
		o.mu.Lock()
		o.runCache[r.ID] = ro
		o.mu.Unlock()
	}
	return ro
}

// Host observes one host against sha.
func (o *Observer) Host(ctx context.Context, h config.Host, sha string, refs Refs) HostObs {
	obs := HostObs{Host: h, Now: time.Now()}
	if !h.AutoUpdating() || !h.Observable() {
		return obs
	}
	st, err := hostprobe.Probe(ctx, h.Name, h.SSH)
	if err != nil {
		obs.Err = err
		return obs
	}
	obs.State = &st
	obs.HeadContains = o.Contains(ctx, sha, st.Head)
	obs.BuiltContains = o.Contains(ctx, sha, st.BuiltCommit)
	obs.AttemptContains = o.Contains(ctx, sha, st.AttemptCommit)
	obs.AttemptIsTip = st.AttemptCommit != "" && st.AttemptCommit == refs.AutoUpdate
	return obs
}

// Journal fetches the log of one auto-update run on a host.
func (o *Observer) Journal(ctx context.Context, h config.Host, invocation string) string {
	out, err := hostprobe.Journal(ctx, h.SSH, invocation, 60)
	if err != nil {
		return ""
	}
	if len(out) > 6000 {
		out = "…" + out[len(out)-6000:]
	}
	return out
}

func gitCommitTime(ctx context.Context, r gitx.Repo, sha string) (time.Time, error) {
	s, err := r.Format(ctx, sha, "%ct")
	if err != nil {
		return time.Time{}, err
	}
	n, err := strconv.ParseInt(strings.TrimSpace(s), 10, 64)
	if err != nil {
		return time.Time{}, err
	}
	return time.Unix(n, 0), nil
}
