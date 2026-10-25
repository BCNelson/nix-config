package app

import (
	"context"
	"encoding/json"
	"fmt"
	mathrand "math/rand/v2"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"nixcfg-mcp/internal/config"
	"nixcfg-mcp/internal/gitx"
	"nixcfg-mcp/internal/pipeline"
)

// The status line shows where the newest commits on origin/main are in the
// deploy pipeline, without anyone running a wait. Working that out means the
// Forgejo API and ssh to every host, far too slow for a command Claude runs
// every few seconds, so it is cached:
//
//   - every run reads origin/main from local refs (no network). If that is
//     the head the cache was built for and every commit in it has settled,
//     it prints the cache and does nothing else;
//   - a new head, a commit still moving (CI running, hosts building), or a
//     fetch that is due starts a detached `nixcfg statusline --refresh`,
//     which writes the cache for a later run to pick up;
//   - a running `--until live` wait on one of the cached commits writes its
//     entry itself on every poll (NoteWait), so while waits cover every
//     moving commit no refresh duplicates their probes;
//   - only the refresh fetches, at most every statusFetchEvery: until then a
//     commit pushed from elsewhere is invisible, but our own pushes update
//     origin/main locally and show up at once.
const (
	statusFetchEvery   = 30 * time.Minute
	statusRecheckEvery = 15 * time.Second // while a cached commit is still moving
	statusRefreshLimit = 2 * time.Minute
	// How far back from origin/main to look for commits in the pipeline, and
	// how many of them to show.
	statusScanCommits = 8
	statusMaxShown    = 3
)

// statusEntry is one commit's status.
type statusEntry struct {
	SHA   string         `json:"sha"`
	State pipeline.State `json:"state"`
	Line  string         `json:"line"`
	// ObservedAt is when the status was observed; an older observation never
	// replaces a newer one (see observe).
	ObservedAt time.Time `json:"observed_at"`
	// CI is set while the commit is in CI with an estimate: the time left is
	// worked out on every redraw (line), so a seconds countdown keeps moving
	// between the refreshes that rewrite Line.
	CI *pipeline.CIInfo `json:"ci,omitempty"`
}

func newStatusEntry(r *pipeline.Result, at time.Time) statusEntry {
	e := statusEntry{SHA: r.Spec.SHA, State: r.State, Line: statusRemoteLine(r), ObservedAt: at}
	if inCI(r) && r.CI != nil && !r.CI.Started.IsZero() {
		ci := *r.CI
		e.CI = &ci
	}
	return e
}

type statusCache struct {
	Head string `json:"head"` // the origin/main commit Entries were chosen for
	// Entries are the deploying commits from Head back, newest first: the
	// ones still moving, then the newest settled one (see pipelineCommits).
	Entries   []statusEntry `json:"entries"`
	AttemptAt time.Time     `json:"attempt_at"` // last refresh attempt, successful or not
	FetchedAt time.Time     `json:"fetched_at"` // last fetch attempt
}

func statusCachePath() string { return config.StateDir("statusline.json") }

func loadStatusCache() statusCache {
	var c statusCache
	if raw, err := os.ReadFile(statusCachePath()); err == nil {
		_ = json.Unmarshal(raw, &c)
	}
	return c
}

func (c statusCache) save() {
	raw, err := json.MarshalIndent(c, "", "  ")
	if err != nil {
		return
	}
	p := statusCachePath()
	_ = os.MkdirAll(filepath.Dir(p), 0o700)
	if os.WriteFile(p+".tmp", raw, 0o600) == nil {
		_ = os.Rename(p+".tmp", p)
	}
}

// moving lists the entries whose deploy hasn't settled.
func (c statusCache) moving() []statusEntry {
	var out []statusEntry
	for _, e := range c.Entries {
		if !e.State.Terminal() {
			out = append(out, e)
		}
	}
	return out
}

// line is what the status line shows: every commit still in the pipeline,
// or, when nothing is, the newest one that deployed (or failed to).
func (c statusCache) line(now time.Time) string {
	shown := c.moving()
	if len(shown) == 0 && len(c.Entries) > 0 {
		shown = c.Entries[len(c.Entries)-1:]
	}
	var parts []string
	for _, e := range shown {
		line := e.Line
		if s := e.CI.Remaining(now); s != "" {
			line += " " + cYellow + s + cReset
		}
		parts = append(parts, line)
	}
	return strings.Join(parts, cDim+"  │  "+cReset)
}

// Two locks, so writers never wait on a slow refresh: ".refresh.lock" keeps
// refreshes single-flight and is held for the whole fetch and probe (up to
// statusRefreshLimit); ".lock" guards one read-modify-write of the cache and
// is held for a few milliseconds.
const (
	statusWriteTries  = 6
	statusJitterMin   = 25 * time.Millisecond
	statusJitterRange = 225 * time.Millisecond
)

// update applies fn to the cache under the write lock and saves if fn says
// so. Writers that clash (a wait's poll and a refresh, or two waits) retry
// after a random jitter, so they don't keep colliding in lockstep. It runs
// inside a wait's save, so the worst case is bounded: about 1.5s, then the
// write is dropped and the next poll tries again.
func update(fn func(*statusCache) bool) bool {
	p := statusCachePath()
	_ = os.MkdirAll(filepath.Dir(p), 0o700)
	for try := range statusWriteTries {
		if try > 0 {
			time.Sleep(statusJitterMin + mathrand.N(statusJitterRange))
		}
		lock, err := os.OpenFile(p+".lock", os.O_CREATE|os.O_RDWR, 0o600)
		if err != nil {
			return false
		}
		if syscall.Flock(int(lock.Fd()), syscall.LOCK_EX|syscall.LOCK_NB) != nil {
			lock.Close()
			continue
		}
		c := loadStatusCache()
		if fn(&c) {
			c.save()
		}
		lock.Close() // releases the flock
		return true
	}
	return false
}

// observe records r in its commit's entry unless the entry already holds a
// newer observation: a refresh that started probing before a wait's poll can
// finish after it. A commit with no entry isn't in the pipeline view, and is
// left out.
func (c *statusCache) observe(r *pipeline.Result, at time.Time) bool {
	for i := range c.Entries {
		e := &c.Entries[i]
		if e.SHA != r.Spec.SHA {
			continue
		}
		if at.Before(e.ObservedAt) {
			return false
		}
		*e = newStatusEntry(r, at)
		return true
	}
	return false
}

// stale says why the cache needs a refresh, or "" if it doesn't. covered
// holds the commits a running wait keeps current.
func (c statusCache) stale(head string, covered map[string]bool, now time.Time) string {
	switch {
	case now.Sub(c.FetchedAt) > statusFetchEvery:
		return "fetch due"
	case head != "" && head != c.Head:
		return "new commit"
	}
	for _, e := range c.moving() {
		last := e.ObservedAt
		if c.AttemptAt.After(last) {
			last = c.AttemptAt
		}
		if !covered[e.SHA] && now.Sub(last) > statusRecheckEvery {
			return "still moving"
		}
	}
	return ""
}

// LocalRemoteHead is origin/main as the checkout at dir ("" for the working
// directory) last saw it.
func LocalRemoteHead(dir string) string {
	args := []string{"rev-parse", "--verify", "--quiet", "refs/remotes/origin/main"}
	if dir != "" {
		args = append([]string{"-C", dir}, args...)
	}
	out, err := exec.Command("git", args...).Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

// RemoteStatus is the status line, kicking off a background refresh when the
// cache is stale. Fast: local refs and one file.
func RemoteStatus(self, head string, covered map[string]bool, now time.Time) string {
	c := loadStatusCache()
	if c.stale(head, covered, now) != "" {
		spawnRefresh(self)
	}
	if head != "" && head != c.Head {
		return fmt.Sprintf("main %s %s…%s", gitx.Short(head), cDim, cReset)
	}
	return c.line(now)
}

// spawnRefresh starts the refresh detached, so it outlives this process and
// the status line never waits on it. The refresh takes a lock, so spawning
// one while another runs is a cheap no-op.
func spawnRefresh(self string) {
	cmd := exec.Command(self, "statusline", "--refresh")
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if cmd.Start() == nil {
		_ = cmd.Process.Release()
	}
}

// RefreshStatus fetches if due and recomputes the pipeline view.
func RefreshStatus(ctx context.Context, ap *App, now time.Time) error {
	lock, err := os.OpenFile(statusCachePath()+".refresh.lock", os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return err
	}
	defer lock.Close()
	if syscall.Flock(int(lock.Fd()), syscall.LOCK_EX|syscall.LOCK_NB) != nil {
		return nil // another refresh is running
	}
	ctx, cancel := context.WithTimeout(ctx, statusRefreshLimit)
	defer cancel()

	// Record the attempt up front: if the network is down, the status line
	// keeps spawning refreshes, and each one must not retry the fetch.
	var why string
	fetch := false
	update(func(c *statusCache) bool {
		why = c.stale(LocalRemoteHead(ap.Cfg.Root), nil, now)
		if why == "" {
			return false // a refresh or wait just before us already did it
		}
		c.AttemptAt = now
		if now.Sub(c.FetchedAt) > statusFetchEvery {
			c.FetchedAt, fetch = now, true
		}
		return true
	})
	if why == "" {
		return nil
	}
	if fetch {
		_, _ = ap.Obs.Git.Fetch(ctx, "main", "auto-update")
	}
	head := LocalRemoteHead(ap.Cfg.Root)
	if head == "" {
		return fmt.Errorf("no origin/main in this checkout")
	}
	if c := loadStatusCache(); head == c.Head && len(c.moving()) == 0 {
		return nil // only the fetch was due, and it found nothing new
	}
	started := time.Now()
	entries := ap.pipelineCommits(ctx, head, started)
	update(func(c *statusCache) bool {
		if c.Head == head {
			// Keep anything a wait observed while we were probing.
			for i, e := range entries {
				for _, old := range c.Entries {
					if old.SHA == e.SHA && old.ObservedAt.After(e.ObservedAt) {
						entries[i] = old
					}
				}
			}
		}
		c.Head, c.Entries = head, entries
		return true
	})
	return nil
}

// pipelineCommits walks back from head over the commits that deploy (touch a
// watched path) and returns those still in the pipeline, newest first, plus
// the first settled one: everything older than that has settled too. It also
// stops at the first commit that has reached auto-update, since hosts build
// that commit or a newer one, which covers every older commit. Commits that
// don't deploy are skipped: a docs commit on top says nothing about whether
// the change under it is live.
func (a *App) pipelineCommits(ctx context.Context, head string, at time.Time) []statusEntry {
	shas, err := a.Obs.Git.Commits(ctx, head+"~"+fmt.Sprint(statusScanCommits), head)
	if err != nil || len(shas) == 0 {
		shas = []string{head}
	}
	var out []statusEntry
	for _, sha := range shas {
		if len(out) == statusMaxShown {
			break
		}
		if files, err := a.Obs.Git.CommitFiles(ctx, sha); err == nil {
			if w, _ := a.Obs.Filter.Classify(files); len(w) == 0 {
				continue
			}
		}
		r, _, err := a.Status(ctx, sha, pipeline.Live, nil)
		if err != nil {
			continue
		}
		if carriedBy(r, out) {
			continue // its CI run is a newer commit's, already shown: same wait
		}
		out = append(out, newStatusEntry(r, at))
		if r.State.Terminal() || reached(r, pipeline.AutoUpdateAdvanced) {
			break
		}
	}
	return out
}

// carriedBy reports whether r is still in CI on a run for a commit in shown
// (several commits pushed together share the run for the newest).
func carriedBy(r *pipeline.Result, shown []statusEntry) bool {
	if r.State.Terminal() || reached(r, pipeline.AutoUpdateAdvanced) || r.CI == nil || !r.CI.Superseded {
		return false
	}
	for _, e := range shown {
		if e.SHA == r.CI.RunCommit {
			return true
		}
	}
	return false
}

// inCI: CI has started and auto-update hasn't advanced to the commit yet.
func inCI(r *pipeline.Result) bool {
	return r.State == pipeline.Pending && reached(r, pipeline.CIStarted) && !reached(r, pipeline.AutoUpdateAdvanced)
}

func reached(r *pipeline.Result, m pipeline.Milestone) bool {
	for _, s := range r.Stages {
		if s.Name == m {
			return s.State == pipeline.Succeeded
		}
	}
	return false
}

// CoversRemote reports whether a wait tracks the whole deploy of a commit, so
// its result is that commit's status: a wait for an earlier milestone ends
// "succeeded" long before the commit is live, and one on named hosts says
// nothing about the rest.
func CoversRemote(r pipeline.Result) bool {
	return r.Spec.Until == pipeline.Live && (r.HostSelection == nil || !r.HostSelection.Explicit)
}

// NoteWait writes a wait's latest result into its commit's entry, if the
// pipeline view has one (Store.OnSave).
func NoteWait(root string, r pipeline.Result) {
	if !CoversRemote(r) {
		return
	}
	at := r.Updated
	if at.IsZero() {
		at = time.Now()
	}
	update(func(c *statusCache) bool {
		if c.Head != LocalRemoteHead(root) {
			return false // origin/main has moved on; a refresh will rebuild the view
		}
		return c.observe(&r, at)
	})
}

func statusRemoteLine(r *pipeline.Result) string {
	head := "main " + gitx.Short(r.Spec.SHA)
	switch r.State {
	case pipeline.Succeeded:
		return fmt.Sprintf("%s %s✓ live%s", head, cGreen, cReset)
	case pipeline.WillNotHappen:
		return fmt.Sprintf("%s %sno deploy%s", head, cDim, cReset)
	case pipeline.Partial:
		return fmt.Sprintf("%s %s◐ partial%s: %s", head, cYellow, cReset, clip(r.Reason))
	case pipeline.Pending:
		// Until auto-update advances no host has anything to build, so the
		// per-host view is a row of "waiting" (plus any host that deploys
		// main directly, like ryuu, jumping ahead). Name what it's waiting on.
		switch {
		case !reached(r, pipeline.CIStarted):
			return fmt.Sprintf("%s %s⏳ waiting for CI%s", head, cYellow, cReset)
		case inCI(r):
			// The time left is added on each redraw (statusCache.line).
			return fmt.Sprintf("%s %s⏳ CI running%s", head, cYellow, cReset)
		}
		if hosts := statusHosts(*r); hosts != "" {
			return fmt.Sprintf("%s %s⏳%s %s", head, cYellow, cReset, hosts)
		}
		return fmt.Sprintf("%s %s⏳ deploying%s", head, cYellow, cReset)
	}
	return fmt.Sprintf("%s %s✗ %s%s: %s", head, cRed, r.State, cReset, clip(r.Reason))
}
