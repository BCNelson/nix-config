// Command nixcfg (also installed as nixcfg-mcp) is agent tooling for the
// nix-config repository: deploy-timeline waits, host and CI status, durable
// build jobs, secret checks, the guard hook, and an MCP server exposing the
// same operations. See AGENTS.md.
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"nixcfg-mcp/internal/app"
	"nixcfg-mcp/internal/guard"
	"nixcfg-mcp/internal/humancmd"
	"nixcfg-mcp/internal/jobs"
	"nixcfg-mcp/internal/pipeline"
)

const version = "0.1.0"

const usage = `nixcfg: agent tooling for nix-config (see AGENTS.md)

  nixcfg wait <rev> [--until M] [--host H]... [--timeout 60m] [--keep-going] [--json]
  nixcfg wait --id <wait-id>          follow (or resume) an existing wait
  nixcfg wait job <job-id> [--timeout 2h]
  nixcfg status [rev] [--until M] [--host H]...
  nixcfg waits                        recent waits
  nixcfg hosts [--probe | --check]     --check: compare .agents/hosts.json with the flake
  nixcfg host-status <host> [--log]
  nixcfg explain [rev | a..b]
  nixcfg check-host <host> [--eval | --build] [--wait]
  nixcfg check-host --all [--wait]
  nixcfg preseed <host> [--wait]
  nixcfg jobs | nixcfg job <job-id> [--cancel]
  nixcfg secrets-status [host | all]
  nixcfg human <action> [host] [arg]  (actions: ` + "%s" + `)
  nixcfg guard [--agent claude|codex|opencode|pi|plain] < payload.json
  nixcfg serve                        MCP server on stdio

Milestones: %s
Exit codes for wait: 0 succeeded, 1 failed, 2 usage, 3 timed out, 4 unreachable,
5 will not happen, 6 blocked on the human.
Global: --repo <path> (default $NIXCFG_REPO or the enclosing git checkout), --json.
`

func main() {
	os.Exit(run(os.Args[1:]))
}

func printUsage() {
	var ms []string
	for _, m := range pipeline.AllMilestones {
		ms = append(ms, string(m))
	}
	fmt.Fprintf(os.Stderr, usage, strings.Join(humancmd.ActionNames(), ", "), strings.Join(ms, " -> "))
}

func run(argv []string) int {
	if filepath.Base(os.Args[0]) == "nixcfg-mcp" && len(argv) == 0 {
		argv = []string{"serve"}
	}
	if len(argv) == 0 || argv[0] == "-h" || argv[0] == "--help" || argv[0] == "help" {
		printUsage()
		return 2
	}
	cmd, rest := argv[0], argv[1:]
	a := parseArgs(rest, map[string]bool{
		"until": true, "host": true, "timeout": true, "id": true, "repo": true, "agent": true,
	})

	switch cmd {
	case "version", "--version":
		fmt.Println(version)
		return 0
	case "guard":
		if a.has("rules-version") {
			fmt.Println(guard.RulesVersion)
			return 0
		}
		return guard.RunHook(a.get("agent", "plain"), os.Stdin, os.Stdout, os.Stderr)
	}

	ap, err := app.New(a.get("repo", ""))
	if err != nil {
		fmt.Fprintln(os.Stderr, "nixcfg:", err)
		return 2
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	switch cmd {
	case "serve":
		if err := serve(ap); err != nil {
			fmt.Fprintln(os.Stderr, "nixcfg-mcp:", err)
			return 1
		}
		return 0
	case "wait":
		return cmdWait(ctx, ap, a)
	case "status":
		until, err := pipeline.ParseMilestone(a.get("until", ""))
		if err != nil {
			return usageErr(err)
		}
		r, _, err := ap.Status(ctx, a.pos(0), until, a.all("host"))
		if err != nil {
			return fail(err)
		}
		return emit(a, r, app.RenderResult(r))
	case "waits":
		rs := ap.Store.List(20)
		if a.has("json") {
			return emit(a, rs, "")
		}
		for _, r := range rs {
			fmt.Printf("%s %-14s %s -> %-20s %s\n", r.ID, r.State, short(r.Spec.SHA), r.Spec.Until, r.Reason)
		}
		return 0
	case "hosts":
		if a.has("check") {
			drift, err := ap.CheckHostsFile(ctx)
			if err != nil {
				return fail(err)
			}
			if a.has("json") {
				return emit(a, drift, "")
			}
			if len(drift) == 0 {
				fmt.Println(".agents/hosts.json matches the flake")
				return 0
			}
			for _, d := range drift {
				fmt.Printf("%-12s %-12s hosts.json=%q flake=%q\n", d.Host, d.Field, d.HostsJSON, d.Flake)
			}
			return 1
		}
		rows := ap.Hosts(ctx, a.has("probe"))
		return emit(a, rows, app.RenderHosts(rows))
	case "host-status":
		if a.pos(0) == "" {
			return usageErr(fmt.Errorf("host-status needs a host"))
		}
		r, err := ap.HostStatus(ctx, a.pos(0), a.has("log"))
		if err != nil {
			return fail(err)
		}
		return emit(a, r, app.RenderHostReport(r))
	case "explain":
		e, err := ap.Explain(ctx, a.pos(0))
		if err != nil {
			return fail(err)
		}
		return emit(a, e, app.RenderExplanation(e))
	case "check-host":
		mode := "dry-run"
		if a.has("eval") {
			mode = "eval"
		}
		if a.has("build") {
			mode = "build"
		}
		j, err := ap.CheckHost(a.pos(0), mode, a.has("all") || a.pos(0) == "all")
		if err != nil {
			return fail(err)
		}
		return jobResult(ctx, a, j)
	case "preseed":
		j, err := ap.Preseed(ctx, a.pos(0))
		if err != nil {
			return fail(err)
		}
		return jobResult(ctx, a, j)
	case "jobs":
		js := jobs.List(30)
		return emit(a, js, app.RenderJobs(js))
	case "job":
		id := a.pos(0)
		var j *jobs.Job
		var err error
		if a.has("cancel") {
			j, err = jobs.Cancel(id)
		} else {
			j, err = jobs.Get(id)
		}
		if err != nil {
			return fail(err)
		}
		return emit(a, j, app.RenderJob(j))
	case "secrets-status":
		reps, err := ap.SecretsStatus(ctx, a.pos(0))
		if err != nil {
			return fail(err)
		}
		return emit(a, reps, app.RenderSecrets(reps))
	case "human":
		act, err := humancmd.Build(ap.Cfg, a.pos(0), a.pos(1), a.pos(2))
		if err != nil {
			return usageErr(err)
		}
		return emit(a, act, app.RenderHuman(act))
	}
	printUsage()
	return 2
}

func cmdWait(ctx context.Context, ap *app.App, a args) int {
	timeout, err := a.duration("timeout", 60*time.Minute)
	if err != nil {
		return usageErr(err)
	}
	if a.pos(0) == "job" {
		return waitJob(ctx, a, a.pos(1), timeout)
	}
	var h *pipeline.Handle
	if id := a.get("id", ""); id != "" {
		h, err = ap.Store.Get(id)
		if err != nil {
			return fail(err)
		}
	} else {
		until, err := pipeline.ParseMilestone(a.get("until", ""))
		if err != nil {
			return usageErr(err)
		}
		sha, err := ap.ResolveRev(ctx, a.pos(0))
		if err != nil {
			return usageErr(err)
		}
		w, err := ap.Store.Start(pipeline.Spec{SHA: sha, Until: until, Hosts: a.all("host"), KeepGoing: a.has("keep-going"), Timeout: timeout})
		if err != nil {
			return fail(err)
		}
		h, _ = ap.Store.Get(w.ID())
	}
	jsonOut := a.has("json")
	enc := json.NewEncoder(os.Stdout)
	r, _, err := h.Await(ctx, 0, 1000*time.Hour, func(e pipeline.Event) {
		if jsonOut {
			_ = enc.Encode(e)
		} else {
			fmt.Println(e.Line())
		}
	})
	if err != nil {
		return fail(err)
	}
	if !r.State.Terminal() {
		fmt.Fprintf(os.Stderr, "interrupted; the wait is saved. Resume with: nixcfg wait --id %s\n", r.ID)
		return 130
	}
	if jsonOut {
		_ = enc.Encode(r)
	} else {
		fmt.Print("\n" + app.RenderResult(&r))
	}
	return r.State.ExitCode()
}

func waitJob(ctx context.Context, a args, id string, timeout time.Duration) int {
	j, err := jobs.Await(ctx, id, timeout)
	if err != nil {
		return fail(err)
	}
	emit(a, j, app.RenderJob(j))
	switch j.Status {
	case jobs.Succeeded:
		return 0
	case jobs.Running:
		return 3
	}
	return 1
}

func jobResult(ctx context.Context, a args, j *jobs.Job) int {
	if a.has("wait") {
		t, err := a.duration("timeout", 3*time.Hour)
		if err != nil {
			return usageErr(err)
		}
		return waitJob(ctx, a, j.ID, t)
	}
	return emit(a, j, app.RenderJob(j))
}

func emit(a args, v any, text string) int {
	if a.has("json") || text == "" {
		enc := json.NewEncoder(os.Stdout)
		enc.SetIndent("", "  ")
		_ = enc.Encode(v)
		return 0
	}
	fmt.Print(text)
	return 0
}

func fail(err error) int {
	fmt.Fprintln(os.Stderr, "nixcfg:", err)
	return 1
}

func usageErr(err error) int {
	fmt.Fprintln(os.Stderr, "nixcfg:", err)
	return 2
}

func short(s string) string {
	if len(s) > 10 {
		return s[:10]
	}
	return s
}

// ---- argument parsing ------------------------------------------------------

// args accepts flags anywhere: --name, --name=value, --name value (for flags
// declared as taking a value). Repeatable flags collect every value.
type args struct {
	flags map[string][]string
	posit []string
}

func parseArgs(argv []string, valued map[string]bool) args {
	a := args{flags: map[string][]string{}}
	for i := 0; i < len(argv); i++ {
		s := argv[i]
		if s == "--" {
			a.posit = append(a.posit, argv[i+1:]...)
			break
		}
		if strings.HasPrefix(s, "--") || (strings.HasPrefix(s, "-") && len(s) == 2) {
			name := strings.TrimLeft(s, "-")
			val := ""
			if n, v, ok := strings.Cut(name, "="); ok {
				name, val = n, v
			} else if valued[name] && i+1 < len(argv) {
				i++
				val = argv[i]
			}
			a.flags[name] = append(a.flags[name], val)
			continue
		}
		a.posit = append(a.posit, s)
	}
	return a
}

func (a args) has(name string) bool { _, ok := a.flags[name]; return ok }

func (a args) get(name, def string) string {
	if v := a.flags[name]; len(v) > 0 && v[len(v)-1] != "" {
		return v[len(v)-1]
	}
	return def
}

func (a args) all(name string) []string {
	var out []string
	for _, v := range a.flags[name] {
		for _, p := range strings.Split(v, ",") {
			if p = strings.TrimSpace(p); p != "" {
				out = append(out, p)
			}
		}
	}
	return out
}

func (a args) pos(i int) string {
	if i < len(a.posit) {
		return a.posit[i]
	}
	return ""
}

func (a args) duration(name string, def time.Duration) (time.Duration, error) {
	v := a.get(name, "")
	if v == "" {
		return def, nil
	}
	if d, err := time.ParseDuration(v); err == nil {
		return d, nil
	}
	return 0, fmt.Errorf("--%s: %q is not a duration (e.g. 45m, 2h)", name, v)
}
