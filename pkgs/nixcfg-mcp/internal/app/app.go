// Package app holds the operations the CLI and the MCP server share. Each
// returns structured data plus a plain-text rendering for agents.
package app

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"sort"
	"strings"
	"sync"
	"time"

	"nixcfg-mcp/internal/config"
	"nixcfg-mcp/internal/gitx"
	"nixcfg-mcp/internal/hostprobe"
	"nixcfg-mcp/internal/humancmd"
	"nixcfg-mcp/internal/jobs"
	"nixcfg-mcp/internal/pipeline"
	"nixcfg-mcp/internal/secrets"
	"nixcfg-mcp/internal/watch"
)

type watchMatch = watch.Match

// App is the loaded repository context.
type App struct {
	Cfg   *config.Config
	Obs   *pipeline.Observer
	Store *pipeline.Store
}

// New loads the repository at root ("" to discover it).
func New(root string) (*App, error) {
	r, err := config.FindRoot(root)
	if err != nil {
		return nil, err
	}
	cfg, err := config.Load(r)
	if err != nil {
		return nil, err
	}
	obs, err := pipeline.NewObserver(cfg)
	if err != nil {
		return nil, err
	}
	st, err := pipeline.NewStore(cfg, obs)
	if err != nil {
		return nil, err
	}
	return &App{Cfg: cfg, Obs: obs, Store: st}, nil
}

func (a *App) git() gitx.Repo { return a.Obs.Git }

// ResolveRev turns "", "HEAD", a branch or a sha into a full commit id.
func (a *App) ResolveRev(ctx context.Context, rev string) (string, error) {
	if rev == "" {
		rev = "HEAD"
	}
	sha, err := a.git().RevParse(ctx, rev)
	if err != nil || sha == "" {
		return "", fmt.Errorf("unknown revision %q", rev)
	}
	return sha, nil
}

// ---- deploy status --------------------------------------------------------

// Status is one observation of the timeline without waiting.
func (a *App) Status(ctx context.Context, rev string, until pipeline.Milestone, hosts []string) (*pipeline.Result, []pipeline.Event, error) {
	sha, err := a.ResolveRev(ctx, rev)
	if err != nil {
		return nil, nil, err
	}
	w, err := pipeline.NewWait(a.Cfg, a.Obs, nil, pipeline.Spec{SHA: sha, Until: until, Hosts: hosts, KeepGoing: true, Timeout: time.Hour})
	if err != nil {
		return nil, nil, err
	}
	w.Step(ctx)
	r, evs := w.Snapshot(0)
	return &r, evs, nil
}

// ---- hosts ----------------------------------------------------------------

// HostRow is one host for hosts_list.
type HostRow struct {
	config.Host
	Name       string           `json:"name"`
	Observable bool             `json:"observable"`
	Live       *hostprobe.State `json:"live,omitempty"`
	ProbeError string           `json:"probe_error,omitempty"`
}

// Hosts lists hosts, optionally probing the observable ones.
func (a *App) Hosts(ctx context.Context, probe bool) []HostRow {
	var rows []HostRow
	for _, n := range a.Cfg.HostNames() {
		h := a.Cfg.Hosts[n]
		rows = append(rows, HostRow{Host: h, Name: n, Observable: h.Observable()})
	}
	if probe {
		var wg sync.WaitGroup
		for i := range rows {
			if !rows[i].Observable || !rows[i].AutoUpdating() {
				continue
			}
			wg.Add(1)
			go func(r *HostRow) {
				defer wg.Done()
				s, err := hostprobe.Probe(ctx, r.Name, r.SSH)
				if err != nil {
					r.ProbeError = err.Error()
					return
				}
				r.Live = &s
			}(&rows[i])
		}
		wg.Wait()
	}
	return rows
}

// HostReport is host_status's result.
type HostReport struct {
	Host       string           `json:"host"`
	State      *hostprobe.State `json:"state,omitempty"`
	AutoUpdate string           `json:"auto_update_tip"`
	UpToDate   bool             `json:"up_to_date"`
	Diagnosis  []string         `json:"diagnosis"`
	Human      []string         `json:"human_actions,omitempty"`
	Journal    string           `json:"journal,omitempty"`
}

// HostStatus probes a host and explains what it sees.
func (a *App) HostStatus(ctx context.Context, name string, withLog bool) (*HostReport, error) {
	h, err := a.Cfg.Host(name)
	if err != nil {
		return nil, err
	}
	rep := &HostReport{Host: h.Name}
	if !h.AutoUpdating() {
		rep.Diagnosis = append(rep.Diagnosis, "not auto-updating: "+h.Note)
		return rep, nil
	}
	if !h.Observable() {
		rep.Diagnosis = append(rep.Diagnosis, "unobservable: no ssh destination in .agents/hosts.json")
		return rep, nil
	}
	refs, rerr := a.Obs.Refs(ctx)
	s, err := hostprobe.Probe(ctx, h.Name, h.SSH)
	if err != nil {
		if err == hostprobe.ErrNeedsLogin {
			rep.Human = append(rep.Human, humancmd.SSHLogin(h))
		}
		return nil, fmt.Errorf("probing %s: %w", h.Name, err)
	}
	rep.State = &s
	rep.AutoUpdate = refs.AutoUpdate
	d := &rep.Diagnosis
	add := func(f string, args ...any) { *d = append(*d, fmt.Sprintf(f, args...)) }

	switch {
	case !s.Enabled:
		add("auto-update.service is not installed")
	case s.Branch == "":
		add("checkout %s is on a detached HEAD: auto-update refuses to run", s.Checkout)
		rep.Human = append(rep.Human, humancmd.SwitchBranch(h, s.Checkout))
	case s.Branch != "auto-update":
		add("checkout %s is on branch %s, not auto-update: it deploys that branch", s.Checkout, s.Branch)
		rep.Human = append(rep.Human, humancmd.SwitchBranch(h, s.Checkout))
	}
	if !s.Unlocked {
		add("git-crypt looks locked in %s: auto-update refuses to run while it is", s.Checkout)
	}
	if rerr == nil && s.BuiltCommit != "" {
		if s.BuiltCommit == refs.AutoUpdate {
			rep.UpToDate = true
			add("up to date: built %s, the auto-update tip", gitx.Short(s.BuiltCommit))
		} else if a.Obs.Contains(ctx, refs.AutoUpdate, s.BuiltCommit) == pipeline.Yes {
			rep.UpToDate = true
			add("built %s, which is ahead of auto-update (%s)", gitx.Short(s.BuiltCommit), gitx.Short(refs.AutoUpdate))
		} else {
			n, _ := a.git().Commits(ctx, s.BuiltCommit, refs.AutoUpdate)
			add("behind: built %s, auto-update is at %s (%d commits ahead)", gitx.Short(s.BuiltCommit), gitx.Short(refs.AutoUpdate), len(n))
		}
	}
	switch {
	case s.Running():
		add("auto-update is running now (since %s), attempt %d/%d of %s", s.StartedAt.Local().Format("15:04:05"), s.AttemptCount, s.MaxRetries, gitx.Short(s.AttemptCommit))
	case s.AttemptCommit != "" && s.AttemptCount >= s.MaxRetries:
		add("retries exhausted: %s failed %d/%d times; the host won't try it again until a new commit reaches auto-update", gitx.Short(s.AttemptCommit), s.AttemptCount, s.MaxRetries)
		rep.Human = append(rep.Human, humancmd.ResetRetries(h))
	case s.AttemptCommit != "":
		add("last attempt at %s failed (%d/%d); it retries on the next run", gitx.Short(s.AttemptCommit), s.AttemptCount, s.MaxRetries)
	case s.Result != "" && s.Result != "success":
		add("the last run (%s) failed before building: %s", s.ExitedAt.Local().Format("Jan 2 15:04"), s.Result)
	default:
		if !s.ExitedAt.IsZero() {
			add("last run finished %s ago (%s)", time.Since(s.ExitedAt).Round(time.Minute), s.Result)
		}
	}
	if s.SwitchPending() {
		if s.Reboot {
			add("a built system is staged for the next boot (switch was inhibited); the host should reboot itself")
		} else {
			add("a built system is staged for the next boot and this host doesn't reboot itself")
			rep.Human = append(rep.Human, humancmd.Reboot(h))
		}
	} else if s.KernelChanged {
		if s.RebootScheduled {
			add("reboot scheduled (kernel/initrd changed)")
		} else if s.Reboot {
			add("kernel/initrd changed since boot; the next successful auto-update reboots it")
		} else {
			add("kernel/initrd changed since boot and this host doesn't reboot itself")
			rep.Human = append(rep.Human, humancmd.Reboot(h))
		}
	}
	if s.Ntfy {
		add("ntfy subscriber active: starts within seconds of auto-update advancing")
	} else if !s.NextTimer.IsZero() {
		add("no ntfy subscriber; next timer run %s (in %s)", s.NextTimer.Local().Format("Jan 2 15:04"), time.Until(s.NextTimer).Round(time.Minute))
	}
	if s.SystemState != "running" {
		add("systemd says %s", s.SystemState)
	}
	if withLog || (s.Result != "" && s.Result != "success") {
		rep.Journal, _ = hostprobe.Journal(ctx, h.SSH, s.Invocation, 80)
	}
	return rep, nil
}

// ---- explain --------------------------------------------------------------

// CommitInfo is one commit's CI relevance.
type CommitInfo struct {
	SHA       string   `json:"sha"`
	Subject   string   `json:"subject"`
	Watched   []string `json:"watched,omitempty"`
	Unwatched []string `json:"unwatched,omitempty"`
}

// Explanation is explain_change's result.
type Explanation struct {
	Range      string       `json:"range"`
	Commits    []CommitInfo `json:"commits,omitempty"`
	Working    *CommitInfo  `json:"working_tree,omitempty"`
	TriggersCI bool         `json:"triggers_ci"`
	Summary    string       `json:"summary"`
	NextBump   time.Time    `json:"next_flake_bump"`
}

// Explain says whether commits (default: origin/main..HEAD plus the working
// tree) will run CI and so deploy.
func (a *App) Explain(ctx context.Context, rev string) (*Explanation, error) {
	g := a.git()
	ex := &Explanation{NextBump: pipeline.NextBump(time.Now())}
	var list []string
	switch {
	case rev == "":
		_, _ = g.Fetch(ctx, "main")
		ex.Range = "origin/main..HEAD (+ working tree)"
		list, _ = g.Commits(ctx, "origin/main", "HEAD")
		if paths, err := g.WorkingChanges(ctx); err == nil && len(paths) > 0 {
			w, u := a.Obs.Filter.Classify(paths)
			ex.Working = &CommitInfo{SHA: "working tree", Watched: matchPaths(w), Unwatched: matchPaths(u)}
		}
	case strings.Contains(rev, ".."):
		ex.Range = rev
		from, to, _ := strings.Cut(rev, "..")
		var err error
		list, err = g.Commits(ctx, from, to)
		if err != nil {
			return nil, err
		}
	default:
		sha, err := a.ResolveRev(ctx, rev)
		if err != nil {
			return nil, err
		}
		ex.Range = gitx.Short(sha)
		list = []string{sha}
	}
	for _, c := range list {
		files, err := g.CommitFiles(ctx, c)
		if err != nil {
			continue
		}
		w, u := a.Obs.Filter.Classify(files)
		ci := CommitInfo{SHA: c, Subject: g.Subject(ctx, c), Watched: matchPaths(w), Unwatched: matchPaths(u)}
		ex.Commits = append(ex.Commits, ci)
		if len(ci.Watched) > 0 {
			ex.TriggersCI = true
		}
	}
	wtWatched := ex.Working != nil && len(ex.Working.Watched) > 0
	switch {
	case ex.TriggersCI:
		ex.Summary = "a push runs CI (check.yml); once check-hosts passes, auto-update advances and hosts deploy it"
	case len(ex.Commits) == 0 && wtWatched:
		ex.Summary = "nothing committed yet; the working tree touches watched paths, so a commit of it will run CI"
	case len(ex.Commits) == 0:
		ex.Summary = "nothing to push"
	default:
		ex.Summary = fmt.Sprintf("no watched path is touched: a push runs no CI and deploys nothing until a later watched change or the daily flake bump (%s UTC)", ex.NextBump.UTC().Format("Jan 2 15:04"))
	}
	return ex, nil
}

func matchPaths(ms []watchMatch) []string {
	var out []string
	for _, m := range ms {
		if m.Pattern != "" {
			out = append(out, m.Path+" ("+m.Pattern+")")
		} else {
			out = append(out, m.Path)
		}
	}
	return out
}

// ---- builds ---------------------------------------------------------------

// CheckHost starts an eval / dry-run / build job for one host or all.
func (a *App) CheckHost(host, mode string, all bool) (*jobs.Job, error) {
	if all {
		return jobs.Start("check-all", "", []string{"env", "CHECK_FLAKE=.", "CHECK_JOBS=3", "bash", ".forgejo/scripts/check-hosts.sh"}, a.Cfg.Root, map[string]string{"mode": "dry-run"})
	}
	if host == "" {
		return nil, fmt.Errorf("name a host, or all")
	}
	kind := "nixosConfigurations"
	if _, ok := a.Cfg.Hosts[host]; !ok {
		h, err := a.Cfg.Host(host)
		if err != nil {
			if host != "redo-3" {
				return nil, err
			}
		} else {
			host = h.Name
		}
	}
	if host == "redo-3" {
		kind = "systemConfigs"
	}
	attr := fmt.Sprintf(".#%s.%s.config.system.build.toplevel", kind, host)
	var argv []string
	switch mode {
	case "", "dry-run":
		mode = "dry-run"
		argv = []string{"nix", "build", attr, "--dry-run"}
	case "eval":
		argv = []string{"nix", "eval", "--raw", attr + ".drvPath"}
	case "build":
		argv = []string{"nix", "build", attr, "--no-link", "--print-out-paths", "--print-build-logs"}
	default:
		return nil, fmt.Errorf("mode must be eval, dry-run or build")
	}
	return jobs.Start("check-host", host, argv, a.Cfg.Root, map[string]string{"mode": mode})
}

// Preseed builds a host's system locally and copies it into romeo's store.
func (a *App) Preseed(ctx context.Context, host string) (*jobs.Job, error) {
	h, err := a.Cfg.Host(host)
	if err != nil {
		return nil, err
	}
	if !h.Preseed {
		return nil, fmt.Errorf("pre-seeding %s gains nothing: it builds on itself and doesn't use romeo's store (only %s do)", h.Name, strings.Join(a.preseedHosts(), ", "))
	}
	root := a.Cfg.Root
	if out, _ := exec.CommandContext(ctx, "git", "-C", root, "config", "--get", "filter.git-crypt.smudge").Output(); len(out) == 0 {
		return nil, fmt.Errorf("git-crypt is locked here, so this build wouldn't match the host's (hand the human: just unlock)")
	}
	if out, _ := exec.CommandContext(ctx, "git", "-C", root, "status", "--porcelain", "--untracked-files=no").Output(); len(strings.TrimSpace(string(out))) > 0 {
		return nil, fmt.Errorf("the checkout has uncommitted changes to tracked files; commit first (with a pathspec) so the build matches what the host will build:\n%s", strings.TrimSpace(string(out)))
	}
	head, err := a.ResolveRev(ctx, "HEAD")
	if err != nil {
		return nil, err
	}
	builder := a.Cfg.Builder
	if builder == "" {
		builder = "bcnelson@romeo.b.nel.family"
	}
	attr := fmt.Sprintf(".#nixosConfigurations.%s.config.system.build.toplevel", h.Name)
	script := strings.Join([]string{
		"set -euo pipefail",
		"echo \"building " + h.Name + " at " + gitx.Short(head) + "\"",
		"out=$(nix build " + attr + " --no-link --print-out-paths --print-build-logs)",
		"echo \"built $out\"",
		"export NIX_SSHOPTS='-o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=4'",
		"timeout 2h nix copy --to ssh-ng://" + builder + " \"$out\"",
		"echo \"copied $out to " + builder + "\"",
	}, "\n")
	return jobs.Start("preseed", h.Name, []string{"bash", "-c", script}, root, map[string]string{"commit": head, "builder": builder})
}

func (a *App) preseedHosts() []string {
	var out []string
	for _, n := range a.Cfg.HostNames() {
		if a.Cfg.Hosts[n].Preseed {
			out = append(out, n)
		}
	}
	return out
}

// ---- secrets --------------------------------------------------------------

// SecretsStatus checks one host, or all auto-updating hosts.
func (a *App) SecretsStatus(ctx context.Context, host string) ([]*secrets.Report, error) {
	var names []string
	if host == "" || host == "all" {
		out, err := exec.CommandContext(ctx, "nix", "eval", "--raw", ".#nixosConfigurations", "--apply", `c: builtins.concatStringsSep " " (builtins.attrNames c)`).Output()
		if err != nil {
			return nil, fmt.Errorf("listing hosts: %v", err)
		}
		names = strings.Fields(string(out))
	} else {
		if h, err := a.Cfg.Host(host); err == nil {
			host = h.Name
		}
		names = []string{host}
	}
	sort.Strings(names)
	reps := make([]*secrets.Report, len(names))
	errs := make([]error, len(names))
	sem := make(chan struct{}, 3)
	var wg sync.WaitGroup
	for i, n := range names {
		wg.Add(1)
		go func(i int, n string) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			kind := "nixosConfigurations"
			if n == "redo-3" {
				kind = "systemConfigs"
			}
			reps[i], errs[i] = secrets.Check(ctx, a.Cfg.Root, n, kind)
		}(i, n)
	}
	wg.Wait()
	var out []*secrets.Report
	var failed []string
	for i, r := range reps {
		if errs[i] != nil {
			failed = append(failed, errs[i].Error())
			continue
		}
		out = append(out, r)
	}
	if len(out) == 0 && len(failed) > 0 {
		return nil, fmt.Errorf("%s", strings.Join(failed, "\n"))
	}
	return out, nil
}

// ---- hosts.json drift ---------------------------------------------------------

// HostDrift is one difference between .agents/hosts.json and the flake.
type HostDrift struct {
	Host      string `json:"host"`
	Field     string `json:"field"`
	HostsJSON string `json:"hosts_json"`
	Flake     string `json:"flake"`
}

const autoUpdateExpr = `au: { enable = au.enable; path = if au.enable then au.path else null; reboot = au.reboot; interval = au.refreshInterval; ntfy = au.ntfy-refresh.enable; }`

// CheckHostsFile compares the auto-update fields of .agents/hosts.json with
// each host's evaluated services.bcnelson.autoUpdate.
func (a *App) CheckHostsFile(ctx context.Context) ([]HostDrift, error) {
	out, err := exec.CommandContext(ctx, "nix", "eval", "--raw", ".#nixosConfigurations", "--apply", `c: builtins.concatStringsSep " " (builtins.attrNames c)`).Output()
	if err != nil {
		return nil, fmt.Errorf("listing hosts: %v", err)
	}
	names := strings.Fields(string(out))
	type au struct {
		Enable   bool    `json:"enable"`
		Path     *string `json:"path"`
		Reboot   bool    `json:"reboot"`
		Interval string  `json:"interval"`
		Ntfy     bool    `json:"ntfy"`
	}
	res := make([]*au, len(names))
	errs := make([]error, len(names))
	sem := make(chan struct{}, 3)
	var wg sync.WaitGroup
	for i, n := range names {
		wg.Add(1)
		go func(i int, n string) {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()
			cmd := exec.CommandContext(ctx, "nix", "eval", "--json", ".#nixosConfigurations."+n+".config.services.bcnelson.autoUpdate", "--apply", autoUpdateExpr)
			cmd.Dir = a.Cfg.Root
			b, err := cmd.Output()
			if err != nil {
				errs[i] = fmt.Errorf("%s: %v", n, err)
				return
			}
			var v au
			errs[i] = json.Unmarshal(b, &v)
			res[i] = &v
		}(i, n)
	}
	wg.Wait()
	var drift []HostDrift
	add := func(h, f, j, fl string) {
		if j != fl {
			drift = append(drift, HostDrift{h, f, j, fl})
		}
	}
	for i, n := range names {
		if errs[i] != nil {
			drift = append(drift, HostDrift{n, "eval", "", errs[i].Error()})
			continue
		}
		v := res[i]
		h, ok := a.Cfg.Hosts[n]
		if !ok {
			drift = append(drift, HostDrift{n, "entry", "missing", "in the flake"})
			continue
		}
		add(n, "auto_update", fmt.Sprint(h.AutoUpdating()), fmt.Sprint(v.Enable))
		if !v.Enable {
			continue
		}
		p := ""
		if v.Path != nil {
			p = *v.Path
		}
		add(n, "checkout", h.Checkout, p)
		add(n, "reboot", fmt.Sprint(h.Reboot), fmt.Sprint(v.Reboot))
		add(n, "interval", h.Interval, v.Interval)
		add(n, "ntfy", fmt.Sprint(h.Ntfy), fmt.Sprint(v.Ntfy))
	}
	for _, n := range a.Cfg.HostNames() {
		found := false
		for _, m := range names {
			found = found || m == n
		}
		if !found {
			drift = append(drift, HostDrift{n, "entry", "present", "not in nixosConfigurations"})
		}
	}
	return drift, nil
}
