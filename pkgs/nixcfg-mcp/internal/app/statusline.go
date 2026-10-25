package app

import (
	"regexp"
	"strings"

	"nixcfg-mcp/internal/pipeline"
)

// Rendering for the status line: one short segment for origin/main's newest
// commit (statusRemoteLine in statuscache.go). Running waits don't get their
// own segments; they keep that one current instead.

const statusMaxDetail = 60

const (
	cReset  = "\x1b[0m"
	cDim    = "\x1b[2m"
	cRed    = "\x1b[31m"
	cGreen  = "\x1b[32m"
	cYellow = "\x1b[33m"
)

var attemptRe = regexp.MustCompile(`attempt \d+/\d+`)

// statusHosts is one short token per host once any host has started; before
// auto-update advances they would all just say "waiting".
func statusHosts(r pipeline.Result) string {
	if r.HostSelection == nil {
		return ""
	}
	var toks []string
	started := false
	for _, n := range r.HostSelection.Hosts {
		p := r.Hosts[n]
		if p == nil {
			continue
		}
		if p.Stage != "" {
			started = true
		}
		toks = append(toks, n+" "+statusHost(p))
	}
	if !started {
		return ""
	}
	return strings.Join(toks, " · ")
}

func statusHost(p *pipeline.HostProgress) string {
	switch p.State {
	case pipeline.Succeeded:
		return cGreen + "✓" + cReset
	case pipeline.Failed:
		return cRed + "✗" + cReset
	case pipeline.Unreachable:
		return cDim + "down" + cReset
	case pipeline.Blocked:
		return cYellow + "needs you" + cReset
	case pipeline.Pending, "":
	default:
		return string(p.State)
	}
	stage := strings.TrimPrefix(string(p.Stage), "host_")
	if stage == "" {
		return cDim + "waiting" + cReset
	}
	a := strings.TrimPrefix(attemptRe.FindString(p.Detail), "attempt ")
	switch {
	case p.Stage != pipeline.HostBuilding:
	case strings.Contains(p.Detail, "failed") || (a == "" && strings.HasPrefix(p.Detail, "waiting")):
		// An attempt failed and the host is idle until its next run: saying
		// "building" here hid a failure for an hour.
		return cYellow + strings.TrimSpace("retry "+a) + cReset
	case a != "":
		stage += " " + a
	}
	return stage
}

func clip(s string) string {
	if r := []rune(s); len(r) > statusMaxDetail {
		return string(r[:statusMaxDetail-1]) + "…"
	}
	return s
}
