// Package watch answers "will this change run CI?" from the push path filter
// in .forgejo/workflows/check.yml, read at run time so it never drifts.
package watch

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"gopkg.in/yaml.v3"
)

// Filter is the on.push filter of a workflow.
type Filter struct {
	Branches []string
	Paths    []string
}

// Load reads the push filter of a workflow file.
func Load(root, workflow string) (*Filter, error) {
	raw, err := os.ReadFile(filepath.Join(root, ".forgejo", "workflows", workflow))
	if err != nil {
		return nil, err
	}
	var wf struct {
		On struct {
			Push struct {
				Branches []string `yaml:"branches"`
				Paths    []string `yaml:"paths"`
			} `yaml:"push"`
		} `yaml:"on"`
	}
	if err := yaml.Unmarshal(raw, &wf); err != nil {
		return nil, fmt.Errorf("parsing %s: %w", workflow, err)
	}
	return &Filter{Branches: wf.On.Push.Branches, Paths: wf.On.Push.Paths}, nil
}

// Match is one path checked against the filter.
type Match struct {
	Path    string `json:"path"`
	Pattern string `json:"pattern,omitempty"` // the first pattern it matched; empty if none
}

// Classify splits paths into watched and unwatched, with Actions path-filter
// semantics: * stays within a directory, ** crosses them (also mid-segment, so
// "**.nix" is any .nix file anywhere), and a later "!pattern" re-excludes.
func (f *Filter) Classify(paths []string) (watched, unwatched []Match) {
	for _, p := range paths {
		m := Match{Path: p}
		hit := false
		for _, pat := range f.Paths {
			neg := len(pat) > 0 && pat[0] == '!'
			if neg {
				pat = pat[1:]
			}
			if globRegexp(pat).MatchString(p) {
				hit = !neg
				if hit && m.Pattern == "" {
					m.Pattern = pat
				}
			}
		}
		if len(f.Paths) == 0 {
			hit = true
		}
		if hit {
			watched = append(watched, m)
		} else {
			unwatched = append(unwatched, m)
		}
	}
	return watched, unwatched
}

// globRegexp translates an Actions filter glob to an anchored regexp.
func globRegexp(pat string) *regexp.Regexp {
	var b strings.Builder
	b.WriteString("^")
	for i := 0; i < len(pat); i++ {
		c := pat[i]
		switch c {
		case '*':
			if i+1 < len(pat) && pat[i+1] == '*' {
				i++
				if i+1 < len(pat) && pat[i+1] == '/' {
					i++
					b.WriteString("(?:.*/)?") // "**/" also matches no directory
				} else {
					b.WriteString(".*")
				}
			} else {
				b.WriteString("[^/]*")
			}
		case '?':
			b.WriteString("[^/]")
		case '[':
			j := strings.IndexByte(pat[i:], ']')
			if j < 0 {
				b.WriteString(regexp.QuoteMeta("["))
				continue
			}
			b.WriteString(pat[i : i+j+1])
			i += j
		case '{':
			j := strings.IndexByte(pat[i:], '}')
			if j < 0 {
				b.WriteString(regexp.QuoteMeta("{"))
				continue
			}
			alts := strings.Split(pat[i+1:i+j], ",")
			for k := range alts {
				alts[k] = regexp.QuoteMeta(alts[k])
			}
			b.WriteString("(?:" + strings.Join(alts, "|") + ")")
			i += j
		default:
			b.WriteString(regexp.QuoteMeta(string(c)))
		}
	}
	b.WriteString("$")
	return regexp.MustCompile(b.String())
}
