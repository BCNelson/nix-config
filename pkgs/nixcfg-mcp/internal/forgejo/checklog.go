package forgejo

import (
	"regexp"
	"strings"
)

// HostCheck is one host's result in a check-hosts log.
type HostCheck struct {
	Host    string `json:"host"`
	OK      bool   `json:"ok"`
	Seconds string `json:"seconds,omitempty"`
	// Tail is the end of the host's ::group:: block (its nix output), for
	// failed hosts only.
	Tail string `json:"tail,omitempty"`
}

// CheckLog is a parsed check-hosts log (.forgejo/scripts/check-hosts.sh).
type CheckLog struct {
	Hosts    []HostCheck `json:"hosts"`
	Failed   []string    `json:"failed,omitempty"`
	Finished bool        `json:"finished"` // saw "All hosts evaluated" or "Failed:"
	Tail     string      `json:"tail,omitempty"`
}

var (
	// "2026-10-18T05:02:51.9611174Z charlie-1: ok (71s)"
	stampRe  = regexp.MustCompile(`^\d{4}-\d\d-\d\dT[\d:.]+Z ?`)
	resultRe = regexp.MustCompile(`^([A-Za-z0-9_.-]+): (ok|FAILED) \((\d+)s\)$`)
)

const tailBytes = 4096

// CheckLogParser accumulates a check-hosts log line by line.
type CheckLogParser struct {
	log      CheckLog
	group    string
	groupBuf []string
	failed   map[string]bool
	last     []string
}

func NewCheckLogParser() *CheckLogParser {
	return &CheckLogParser{failed: map[string]bool{}}
}

// Line feeds one log line.
func (p *CheckLogParser) Line(raw string) {
	line := stampRe.ReplaceAllString(raw, "")
	p.last = append(p.last, line)
	if len(p.last) > 40 {
		p.last = p.last[1:]
	}

	switch {
	case strings.HasPrefix(line, "##[group]") || strings.HasPrefix(line, "::group::"):
		p.group = strings.TrimPrefix(strings.TrimPrefix(line, "##[group]"), "::group::")
		p.groupBuf = p.groupBuf[:0]
		return
	case strings.HasPrefix(line, "##[endgroup]") || strings.HasPrefix(line, "::endgroup::"):
		if p.failed[p.group] {
			for i := range p.log.Hosts {
				if p.log.Hosts[i].Host == p.group {
					p.log.Hosts[i].Tail = tailOf(p.groupBuf)
				}
			}
		}
		p.group = ""
		return
	}
	if p.group != "" {
		if p.failed[p.group] {
			p.groupBuf = append(p.groupBuf, line)
			if len(p.groupBuf) > 400 {
				p.groupBuf = p.groupBuf[len(p.groupBuf)-400:]
			}
		}
		return
	}
	if m := resultRe.FindStringSubmatch(line); m != nil {
		ok := m[2] == "ok"
		p.log.Hosts = append(p.log.Hosts, HostCheck{Host: m[1], OK: ok, Seconds: m[3]})
		if !ok {
			p.failed[m[1]] = true
		}
		return
	}
	if strings.HasPrefix(line, "Failed:") {
		p.log.Failed = strings.Fields(strings.TrimPrefix(line, "Failed:"))
		p.log.Finished = true
	}
	if strings.HasPrefix(line, "All hosts evaluated") {
		p.log.Finished = true
	}
}

// Result returns what has been parsed so far.
func (p *CheckLogParser) Result() CheckLog {
	r := p.log
	if len(r.Failed) == 0 {
		for _, h := range r.Hosts {
			if !h.OK {
				r.Failed = append(r.Failed, h.Host)
			}
		}
	}
	r.Tail = tailOf(p.last)
	return r
}

func tailOf(lines []string) string {
	s := strings.Join(lines, "\n")
	if len(s) > tailBytes {
		s = "…" + s[len(s)-tailBytes:]
	}
	return s
}

// Tail keeps the last n bytes of a streamed log.
type Tail struct {
	lines []string
	n     int
}

func NewTail(lines int) *Tail { return &Tail{n: lines} }

func (t *Tail) Line(raw string) {
	t.lines = append(t.lines, stampRe.ReplaceAllString(raw, ""))
	if len(t.lines) > t.n {
		t.lines = t.lines[1:]
	}
}

func (t *Tail) String() string { return tailOf(t.lines) }
