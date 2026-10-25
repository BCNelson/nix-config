package app

import (
	"fmt"
	"sort"
	"strings"
	"time"

	"nixcfg-mcp/internal/gitx"
	"nixcfg-mcp/internal/humancmd"
	"nixcfg-mcp/internal/jobs"
	"nixcfg-mcp/internal/pipeline"
	"nixcfg-mcp/internal/secrets"
)

func mark(s pipeline.State) string {
	switch s {
	case pipeline.Succeeded:
		return "ok  "
	case pipeline.Pending:
		return "..  "
	case pipeline.Failed:
		return "FAIL"
	case pipeline.WillNotHappen:
		return "NEVR"
	case pipeline.Blocked:
		return "HUMN"
	case pipeline.Unreachable:
		return "DOWN"
	case pipeline.TimedOut:
		return "TIME"
	case pipeline.Partial:
		return "PART"
	}
	return "    "
}

// RenderResult is the human-readable form of a wait or status.
func RenderResult(r *pipeline.Result) string {
	var b strings.Builder
	fmt.Fprintf(&b, "%s %q -> %s: %s", gitx.Short(r.Spec.SHA), r.Subject, r.Spec.Until, strings.ToUpper(string(r.State)))
	if r.ID != "" && !r.State.Terminal() {
		fmt.Fprintf(&b, " (wait_id %s, deadline %s)", r.ID, r.Deadline.Local().Format("15:04"))
	}
	b.WriteString("\n")
	for _, s := range r.Stages {
		fmt.Fprintf(&b, "  %s %-21s %s\n", mark(s.State), s.Name, s.Detail)
	}
	if r.CI != nil && r.CI.RunURL != "" {
		fmt.Fprintf(&b, "  ci: %s", r.CI.RunURL)
		if s := r.CI.Remaining(time.Now()); s != "" && !r.State.Terminal() {
			fmt.Fprintf(&b, " (%s)", s)
		}
		b.WriteString("\n")
	}
	if sel := r.HostSelection; sel != nil {
		fmt.Fprintf(&b, "hosts: %s", strings.Join(sel.Hosts, ", "))
		if !sel.Explicit && len(sel.Excluded) > 0 {
			var ex []string
			for _, n := range sortedStrings(sel.Excluded) {
				ex = append(ex, n+" ("+sel.Excluded[n]+")")
			}
			fmt.Fprintf(&b, "  [left out: %s]", strings.Join(ex, ", "))
		}
		b.WriteString("\n")
		for _, n := range sel.Hosts {
			p := r.Hosts[n]
			if p == nil {
				continue
			}
			stage := string(p.Stage)
			if stage == "" {
				stage = "-"
			}
			line := fmt.Sprintf("  %s %-12s %-17s %s", mark(p.State), n, stage, p.Detail)
			if p.ETA != "" && !p.State.Terminal() {
				line += " [" + p.ETA + "]"
			}
			b.WriteString(strings.TrimRight(line, " ") + "\n")
		}
	}
	if r.Reason != "" {
		fmt.Fprintf(&b, "reason: %s\n", r.Reason)
	}
	for _, h := range r.HumanActions {
		fmt.Fprintf(&b, "hand the human (one line): %s\n", h)
	}
	if r.LogTail != "" {
		fmt.Fprintf(&b, "log tail:\n%s\n", indent(clipTail(r.LogTail)))
	}
	if r.State.Terminal() {
		b.WriteString(r.Summary())
		b.WriteString("\n")
	}
	return b.String()
}

// RenderEvents renders progress lines.
func RenderEvents(evs []pipeline.Event) string {
	var b strings.Builder
	for _, e := range evs {
		b.WriteString(e.Line())
		b.WriteString("\n")
	}
	return b.String()
}

// Bounds for the log tail in rendered output. Agents stream this (Monitor,
// MCP), and a journal tail can be one 6 KB line (systemd listing every
// container mount it stopped), which buries the error and tempts agents to
// pipe the whole command through `tail`. --json keeps the full tail.
const (
	tailMaxLines = 40
	tailMaxLine  = 300
)

// clipTail keeps the last tailMaxLines lines and the end of any long line,
// where the error usually is.
func clipTail(s string) string {
	lines := strings.Split(strings.TrimRight(s, "\n"), "\n")
	var out []string
	if n := len(lines) - tailMaxLines; n > 0 {
		out = append(out, fmt.Sprintf("… %d earlier lines (--json has them)", n))
		lines = lines[n:]
	}
	for _, l := range lines {
		if r := []rune(l); len(r) > tailMaxLine {
			l = fmt.Sprintf("…[%d chars] %s", len(r)-tailMaxLine, string(r[len(r)-tailMaxLine:]))
		}
		out = append(out, l)
	}
	return strings.Join(out, "\n")
}

func indent(s string) string {
	lines := strings.Split(strings.TrimRight(s, "\n"), "\n")
	for i := range lines {
		lines[i] = "    " + lines[i]
	}
	return strings.Join(lines, "\n")
}

func sortedStrings(m map[string]string) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

// RenderHosts renders hosts_list.
func RenderHosts(rows []HostRow) string {
	var b strings.Builder
	fmt.Fprintf(&b, "%-12s %-5s %-9s %-4s %-6s %-30s %s\n", "host", "obs", "interval", "ntfy", "reboot", "ssh", "live")
	for _, r := range rows {
		if !r.AutoUpdating() {
			fmt.Fprintf(&b, "%-12s %s\n", r.Name, "not auto-updating: "+r.Note)
			continue
		}
		obs := "no"
		if r.Observable {
			obs = "yes"
		}
		live := ""
		switch {
		case r.ProbeError != "":
			live = "probe failed: " + r.ProbeError
		case r.Live != nil:
			s := r.Live
			live = fmt.Sprintf("%s built=%s %s", s.Branch, gitx.Short(s.BuiltCommit), s.Active)
			if s.AttemptCommit != "" {
				live += fmt.Sprintf(" attempt=%s %d/%d", gitx.Short(s.AttemptCommit), s.AttemptCount, s.MaxRetries)
			}
			if s.NeedsReboot() {
				live += " reboot-needed"
			}
		}
		fmt.Fprintf(&b, "%-12s %-5s %-9s %-4v %-6v %-30s %s\n", r.Name, obs, r.Interval, yn(r.Ntfy), yn(r.Reboot), r.SSH, live)
	}
	return b.String()
}

func yn(b bool) string {
	if b {
		return "yes"
	}
	return "no"
}

// RenderHostReport renders host_status.
func RenderHostReport(r *HostReport) string {
	var b strings.Builder
	fmt.Fprintf(&b, "%s\n", r.Host)
	if s := r.State; s != nil {
		fmt.Fprintf(&b, "  checkout  %s on %s, HEAD %s\n", s.Checkout, orDash(s.Branch), gitx.Short(s.Head))
		fmt.Fprintf(&b, "  state     built=%s attempt=%s count=%d/%d\n", orDash(gitx.Short(s.BuiltCommit)), orDash(gitx.Short(s.AttemptCommit)), s.AttemptCount, s.MaxRetries)
		fmt.Fprintf(&b, "  service   %s (%s), invocation %s\n", s.Active, orDash(s.Result), orDash(s.Invocation))
		fmt.Fprintf(&b, "  system    %s, current %s\n", s.SystemState, trimStore(s.Current))
	}
	for _, d := range r.Diagnosis {
		fmt.Fprintf(&b, "  - %s\n", d)
	}
	for _, h := range r.Human {
		fmt.Fprintf(&b, "hand the human (one line): %s\n", h)
	}
	if r.Journal != "" {
		fmt.Fprintf(&b, "journal (last run):\n%s\n", indent(r.Journal))
	}
	return b.String()
}

func orDash(s string) string {
	if s == "" {
		return "-"
	}
	return s
}

func trimStore(p string) string {
	p = strings.TrimPrefix(p, "/nix/store/")
	if len(p) > 33 {
		return p[33:]
	}
	return p
}

// RenderExplanation renders explain_change.
func RenderExplanation(e *Explanation) string {
	var b strings.Builder
	fmt.Fprintf(&b, "%s: %s\n", e.Range, e.Summary)
	one := func(c CommitInfo) {
		head := c.SHA
		if gitx.IsSHA(head) {
			head = gitx.Short(head) + " " + c.Subject
		}
		state := "no CI"
		if len(c.Watched) > 0 {
			state = "runs CI"
		}
		fmt.Fprintf(&b, "  %s  [%s]\n", head, state)
		for _, w := range c.Watched {
			fmt.Fprintf(&b, "      + %s\n", w)
		}
		for i, u := range c.Unwatched {
			if i == 8 {
				fmt.Fprintf(&b, "      - … %d more unwatched\n", len(c.Unwatched)-8)
				break
			}
			fmt.Fprintf(&b, "      - %s\n", u)
		}
	}
	for _, c := range e.Commits {
		one(c)
	}
	if e.Working != nil {
		one(*e.Working)
	}
	return b.String()
}

// RenderJob renders a job.
func RenderJob(j *jobs.Job) string {
	var b strings.Builder
	exit := ""
	if j.Exit != nil {
		exit = fmt.Sprintf(" exit %d", *j.Exit)
	}
	what := j.Kind
	if j.Host != "" {
		what += " " + j.Host
	}
	fmt.Fprintf(&b, "%s %s: %s%s after %s\n", j.ID, what, strings.ToUpper(string(j.Status)), exit, j.Elapsed)
	fmt.Fprintf(&b, "  %s\n", strings.Join(j.Argv, " "))
	fmt.Fprintf(&b, "  log: %s\n", jobs.LogPath(j.ID))
	if j.Error != "" {
		fmt.Fprintf(&b, "error:\n%s\n", indent(clipTail(j.Error)))
	} else if j.Tail != "" {
		fmt.Fprintf(&b, "tail:\n%s\n", indent(clipTail(j.Tail)))
	}
	if !j.Done() {
		fmt.Fprintf(&b, "still running; follow with: nixcfg wait job %s\n", j.ID)
	}
	return b.String()
}

// RenderJobs renders jobs_list.
func RenderJobs(js []*jobs.Job) string {
	var b strings.Builder
	for _, j := range js {
		fmt.Fprintf(&b, "%s %-10s %-10s %-9s %8s  %s\n", j.ID, j.Kind, orDash(j.Host), j.Status, j.Elapsed, j.Started.Local().Format(time.DateTime))
	}
	if b.Len() == 0 {
		return "no jobs\n"
	}
	return b.String()
}

// RenderSecrets renders secrets_status.
func RenderSecrets(reps []*secrets.Report) string {
	var b strings.Builder
	for _, r := range reps {
		bad := 0
		for _, s := range r.Secrets {
			if s.State != "ok" && s.State != "intermediary" {
				bad++
			}
		}
		fmt.Fprintf(&b, "%s: %d secret(s), %d ok", r.Host, len(r.Secrets), r.OK)
		if bad > 0 {
			fmt.Fprintf(&b, ", %d need attention", bad)
		}
		if len(r.Orphans) > 0 {
			fmt.Fprintf(&b, ", %d orphaned rekeyed file(s)", len(r.Orphans))
		}
		b.WriteString("\n")
		for _, s := range r.Secrets {
			if s.State != "ok" && s.State != "intermediary" {
				fmt.Fprintf(&b, "  %-24s %-22s %s\n", s.Name, s.State, s.RekeyFile)
			}
		}
		for _, o := range r.Orphans {
			fmt.Fprintf(&b, "  orphan %s\n", o)
		}
		for _, f := range r.Fixes {
			who := "you"
			if f.Actor == "human" {
				who = "human"
			}
			touch := ""
			if f.NeedsKeyTouch {
				touch = " (key touch)"
			}
			fmt.Fprintf(&b, "  fix [%s%s]: %s  -- %s\n", who, touch, f.Command, f.Why)
		}
	}
	return b.String()
}

// RenderHuman renders human_command.
func RenderHuman(a humancmd.Action) string {
	touch := ""
	if a.NeedsKeyTouch {
		touch = " (needs a hardware key touch)"
	}
	return fmt.Sprintf("hand the human (one line)%s:\n%s\n(%s)\n", touch, a.Command, a.Why)
}
