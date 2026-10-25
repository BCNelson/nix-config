package app

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"syscall"
	"testing"
	"time"

	"nixcfg-mcp/internal/pipeline"
)

func TestClipTail(t *testing.T) {
	if got := clipTail("a\nb\n"); got != "a\nb" {
		t.Errorf("short tail changed: %q", got)
	}

	var lines []string
	for range tailMaxLines + 5 {
		lines = append(lines, "x")
	}
	got := strings.Split(clipTail(strings.Join(lines, "\n")), "\n")
	if len(got) != tailMaxLines+1 || !strings.HasPrefix(got[0], "… 5 earlier lines") {
		t.Errorf("long tail: %d lines, first %q", len(got), got[0])
	}

	long := strings.Repeat("m", 1000) + "THE ERROR"
	got = strings.Split(clipTail(long), "\n")
	if len(got) != 1 || !strings.HasSuffix(got[0], "THE ERROR") || len([]rune(got[0])) > tailMaxLine+20 {
		t.Errorf("long line not clipped to its end: %q", got[0])
	}
}

func TestStatusRemoteLine(t *testing.T) {
	sha := "52518bc0000000000000000000000000000000ff"
	strip := regexp.MustCompile("\x1b\\[[0-9;]*m")
	cases := []struct {
		name string
		r    pipeline.Result
		want string
	}{
		{"deploying", pipeline.Result{
			Spec: pipeline.Spec{SHA: sha, Until: pipeline.Live}, State: pipeline.Pending,
			Stages: []pipeline.Stage{
				{Name: pipeline.CIStarted, State: pipeline.Succeeded},
				{Name: pipeline.AutoUpdateAdvanced, State: pipeline.Succeeded},
			},
			HostSelection: &pipeline.HostSelection{Hosts: []string{"romeo-2", "whiskey-1", "vor-2", "ryuu-2"}},
			Hosts: map[string]*pipeline.HostProgress{
				"romeo-2":   {Stage: pipeline.HostBuilding, State: pipeline.Pending, Detail: "building, attempt 2/3"},
				"whiskey-1": {Stage: pipeline.HostRebootedOK, State: pipeline.Succeeded},
				"vor-2":     {State: pipeline.Pending},
				"ryuu-2":    {Stage: pipeline.HostBuilding, State: pipeline.Pending, Detail: "attempt 1/3 failed; waiting for the next run"},
			},
		}, "main 52518bc000 ⏳ romeo-2 building 2/3 · whiskey-1 ✓ · vor-2 waiting · ryuu-2 retry 1/3"},
		// ryuu deploys main directly, so it can finish before CI does: still "CI running".
		{"in CI, one host ahead", pipeline.Result{
			Spec: pipeline.Spec{SHA: sha, Until: pipeline.Live}, State: pipeline.Pending,
			Stages: []pipeline.Stage{
				{Name: pipeline.CIStarted, State: pipeline.Succeeded},
				{Name: pipeline.AutoUpdateAdvanced, State: pipeline.Pending},
			},
			HostSelection: &pipeline.HostSelection{Hosts: []string{"romeo-2", "ryuu-2"}},
			Hosts: map[string]*pipeline.HostProgress{
				"romeo-2": {State: pipeline.Pending},
				"ryuu-2":  {Stage: pipeline.HostRebootedOK, State: pipeline.Succeeded},
			},
		}, "main 52518bc000 ⏳ CI running"},
		{"no CI run yet", pipeline.Result{Spec: pipeline.Spec{SHA: sha}, State: pipeline.Pending,
			Stages: []pipeline.Stage{{Name: pipeline.CIStarted, State: pipeline.Pending}}}, "main 52518bc000 ⏳ waiting for CI"},
		{"live", pipeline.Result{Spec: pipeline.Spec{SHA: sha}, State: pipeline.Succeeded}, "main 52518bc000 ✓ live"},
		{"failed", pipeline.Result{Spec: pipeline.Spec{SHA: sha}, State: pipeline.Failed, Reason: "romeo-2: build failed 3/3 times"}, "main 52518bc000 ✗ failed: romeo-2: build failed 3/3 times"},
	}
	for _, c := range cases {
		if got := strip.ReplaceAllString(statusRemoteLine(&c.r), ""); got != c.want {
			t.Errorf("%s:\n got  %q\n want %q", c.name, got, c.want)
		}
	}
}

func TestCarriedBy(t *testing.T) {
	inCI := []pipeline.Stage{{Name: pipeline.CIStarted, State: pipeline.Succeeded}, {Name: pipeline.AutoUpdateAdvanced, State: pipeline.Pending}}
	shown := []statusEntry{{SHA: "bbb"}}
	riding := &pipeline.Result{State: pipeline.Pending, Stages: inCI, CI: &pipeline.CIInfo{Superseded: true, RunCommit: "bbb"}}
	if !carriedBy(riding, shown) {
		t.Error("a commit on a shown commit's CI run should be folded into it")
	}
	own := &pipeline.Result{State: pipeline.Pending, Stages: inCI, CI: &pipeline.CIInfo{RunCommit: "aaa"}}
	if carriedBy(own, shown) {
		t.Error("a commit with its own run must be shown")
	}
	elsewhere := &pipeline.Result{State: pipeline.Pending, Stages: inCI, CI: &pipeline.CIInfo{Superseded: true, RunCommit: "zzz"}}
	if carriedBy(elsewhere, shown) {
		t.Error("a run for a commit that isn't shown doesn't hide this one")
	}
}

func TestStatusCacheStale(t *testing.T) {
	now := time.Date(2026, 10, 25, 12, 0, 0, 0, time.UTC)
	fresh := now.Add(-time.Minute)
	entry := func(sha string, st pipeline.State, seen time.Time) statusEntry {
		return statusEntry{SHA: sha, State: st, ObservedAt: seen}
	}
	settled := statusCache{Head: "aaa", FetchedAt: fresh, Entries: []statusEntry{entry("aaa", pipeline.Succeeded, now.Add(-time.Hour))}}
	// bbb is in CI, aaa (older) is building on the hosts.
	moving := statusCache{Head: "bbb", FetchedAt: fresh, Entries: []statusEntry{
		entry("bbb", pipeline.Pending, now.Add(-5*time.Second)),
		entry("aaa", pipeline.Pending, now.Add(-time.Minute)),
	}}
	cases := []struct {
		name    string
		c       statusCache
		head    string
		covered map[string]bool
		want    string
	}{
		{"same head, all settled: nothing to do", settled, "aaa", nil, ""},
		{"a push moved origin/main", settled, "ccc", nil, "new commit"},
		{"fetch due", func() statusCache { c := settled; c.FetchedAt = now.Add(-31 * time.Minute); return c }(), "aaa", nil, "fetch due"},
		{"an older commit in the pipeline is due", moving, "bbb", nil, "still moving"},
		{"a wait keeps the due one current", moving, "bbb", map[string]bool{"aaa": true}, ""},
		{"a recent attempt counts as a check", func() statusCache { c := moving; c.AttemptAt = now.Add(-time.Second); return c }(), "bbb", nil, ""},
		{"no cache yet", statusCache{}, "aaa", nil, "fetch due"},
	}
	for _, c := range cases {
		if got := c.c.stale(c.head, c.covered, now); got != c.want {
			t.Errorf("%s: got %q, want %q", c.name, got, c.want)
		}
	}
}

func TestStatusCacheLine(t *testing.T) {
	e := func(sha string, st pipeline.State) statusEntry {
		return statusEntry{SHA: sha, State: st, Line: sha + ":" + string(st)}
	}
	strip := regexp.MustCompile("\x1b\\[[0-9;]*m")
	for _, c := range []struct {
		entries []statusEntry
		want    string
	}{
		{[]statusEntry{e("bbb", pipeline.Pending), e("aaa", pipeline.Pending)}, "bbb:pending  │  aaa:pending"},
		// The settled one is the floor of the walk; it only shows when nothing moves.
		{[]statusEntry{e("bbb", pipeline.Pending), e("aaa", pipeline.Succeeded)}, "bbb:pending"},
		{[]statusEntry{e("aaa", pipeline.Succeeded)}, "aaa:succeeded"},
		{nil, ""},
	} {
		if got := strip.ReplaceAllString(statusCache{Entries: c.entries}.line(time.Time{}), ""); got != c.want {
			t.Errorf("line(%v) = %q, want %q", c.entries, got, c.want)
		}
	}
}

func TestStatusCacheLineCountsDown(t *testing.T) {
	start := time.Date(2026, 10, 25, 16, 0, 0, 0, time.UTC)
	c := statusCache{Entries: []statusEntry{{SHA: "aaa", State: pipeline.Pending, Line: "main aaa ⏳ CI running",
		CI: &pipeline.CIInfo{Started: start, TypicalSec: 288}}}}
	strip := regexp.MustCompile("\x1b\\[[0-9;]*m")
	// Two redraws 5s apart from the same cached entry.
	for _, c2 := range []struct {
		at   time.Duration
		want string
	}{
		{90 * time.Second, "main aaa ⏳ CI running ~3m18s left"},
		{95 * time.Second, "main aaa ⏳ CI running ~3m13s left"},
	} {
		if got := strip.ReplaceAllString(c.line(start.Add(c2.at)), ""); got != c2.want {
			t.Errorf("at +%s: %q, want %q", c2.at, got, c2.want)
		}
	}
}

func TestStatusCacheObserveKeepsNewest(t *testing.T) {
	t0 := time.Date(2026, 10, 25, 12, 0, 0, 0, time.UTC)
	building := &pipeline.Result{Spec: pipeline.Spec{SHA: "aaa", Until: pipeline.Live}, State: pipeline.Pending}
	live := &pipeline.Result{Spec: pipeline.Spec{SHA: "aaa", Until: pipeline.Live}, State: pipeline.Succeeded}

	c := statusCache{Head: "aaa", Entries: []statusEntry{{SHA: "aaa", State: pipeline.Pending}}}
	if !c.observe(live, t0) || c.Entries[0].State != pipeline.Succeeded {
		t.Fatal("first observation not recorded")
	}
	// A refresh that started probing before the wait's poll finishes after it.
	if c.observe(building, t0.Add(-5*time.Second)) || c.Entries[0].State != pipeline.Succeeded {
		t.Errorf("an older observation replaced a newer one: %s", c.Entries[0].State)
	}
	other := &pipeline.Result{Spec: pipeline.Spec{SHA: "zzz", Until: pipeline.Live}, State: pipeline.Pending}
	if c.observe(other, t0.Add(time.Hour)) || len(c.Entries) != 1 {
		t.Error("a commit outside the pipeline view was added")
	}
}

func TestStatusCacheUpdateRetriesOnClash(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	p := statusCachePath()
	_ = os.MkdirAll(filepath.Dir(p), 0o700)
	held, err := os.OpenFile(p+".lock", os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	if err := syscall.Flock(int(held.Fd()), syscall.LOCK_EX); err != nil {
		t.Fatal(err)
	}
	go func() { time.Sleep(60 * time.Millisecond); held.Close() }()

	ok := update(func(c *statusCache) bool { c.Head = "aaa"; return true })
	if !ok || loadStatusCache().Head != "aaa" {
		t.Errorf("write lost after the other writer let go: ok=%v cache=%+v", ok, loadStatusCache())
	}
}
