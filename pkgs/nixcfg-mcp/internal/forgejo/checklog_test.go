package forgejo

import (
	"bufio"
	"os"
	"strings"
	"testing"
)

func TestCheckLogParser(t *testing.T) {
	f, err := os.Open("testdata/check-hosts-failed.log")
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	p := NewCheckLogParser()
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		p.Line(sc.Text())
	}
	r := p.Result()
	if !r.Finished || len(r.Failed) != 1 || r.Failed[0] != "golf-3" {
		t.Fatalf("finished=%v failed=%v", r.Finished, r.Failed)
	}
	if len(r.Hosts) < 15 {
		t.Fatalf("parsed %d hosts", len(r.Hosts))
	}
	for _, h := range r.Hosts {
		if h.Host == "golf-3" {
			if h.OK || !strings.Contains(h.Tail, "attribute 'frobnicate' missing") {
				t.Fatalf("golf-3 = %+v", h)
			}
		} else if !h.OK || h.Tail != "" {
			t.Fatalf("%s = %+v", h.Host, h)
		}
	}
}

func TestStatusParts(t *testing.T) {
	s := Status{Context: "Nix Flake Check / check-hosts (push)", TargetURL: "/bcnelson/nix-config/actions/runs/22/jobs/0"}
	if s.Job() != "check-hosts" || s.Event() != "push" || s.Workflow() != "Nix Flake Check" {
		t.Fatalf("%q %q %q", s.Job(), s.Event(), s.Workflow())
	}
	if run, job, ok := s.RunJob(); !ok || run != 22 || job != 0 {
		t.Fatalf("%d %d %v", run, job, ok)
	}
}
