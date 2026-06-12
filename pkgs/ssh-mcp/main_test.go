package main

import (
	"strings"
	"testing"
)

func TestParseHostSpec(t *testing.T) {
	tests := []struct {
		spec     string
		wantName string
		wantUser string
		wantAddr string
		wantErr  bool
	}{
		{"romeo=bcnelson@romeo.b.nel.family", "romeo", "bcnelson", "romeo.b.nel.family:22", false},
		{"vor=bcnelson@vor.ck.nel.family:2222", "vor", "bcnelson", "vor.ck.nel.family:2222", false},
		{"missingat=bcnelson", "", "", "", true},
		{"=bcnelson@host", "", "", "", true},
		{"name=@host", "", "", "", true},
		{"name=user@", "", "", "", true},
		{"noequals", "", "", "", true},
	}
	for _, tt := range tests {
		got, err := parseHostSpec(tt.spec)
		if tt.wantErr {
			if err == nil {
				t.Errorf("parseHostSpec(%q) = %+v, want error", tt.spec, got)
			}
			continue
		}
		if err != nil {
			t.Errorf("parseHostSpec(%q) unexpected error: %v", tt.spec, err)
			continue
		}
		if got.name != tt.wantName || got.user != tt.wantUser || got.addr != tt.wantAddr {
			t.Errorf("parseHostSpec(%q) = %+v, want {name:%q user:%q addr:%q}",
				tt.spec, got, tt.wantName, tt.wantUser, tt.wantAddr)
		}
	}
}

func TestParseHosts(t *testing.T) {
	hosts, err := parseHosts([]string{
		"--host", "romeo=bcnelson@romeo.b.nel.family",
		"--host=whiskey=bcnelson@whiskey.b.nel.family",
	})
	if err != nil {
		t.Fatalf("parseHosts error: %v", err)
	}
	if len(hosts) != 2 {
		t.Fatalf("got %d hosts, want 2", len(hosts))
	}
	if hosts["romeo"].addr != "romeo.b.nel.family:22" {
		t.Errorf("romeo addr = %q", hosts["romeo"].addr)
	}

	if _, err := parseHosts([]string{"--host", "a=u@h", "--host", "a=u@h2"}); err == nil {
		t.Error("expected duplicate-name error")
	}
	if _, err := parseHosts([]string{"--bogus"}); err == nil {
		t.Error("expected unexpected-argument error")
	}
}

func TestOutputBufferTruncation(t *testing.T) {
	b := newOutputBuffer(10)
	b.Write([]byte("abcdef"))
	b.Write([]byte("ghijkl")) // total 12 > cap 10, keep last 10

	out, truncated, total := b.snapshot()
	if !truncated {
		t.Error("expected truncated = true")
	}
	if total != 12 {
		t.Errorf("total = %d, want 12", total)
	}
	if out != "cdefghijkl" {
		t.Errorf("out = %q, want %q", out, "cdefghijkl")
	}
}

func TestOutputBufferNoTruncation(t *testing.T) {
	b := newOutputBuffer(10)
	b.Write([]byte("hello"))
	out, truncated, total := b.snapshot()
	if truncated || total != 5 || out != "hello" {
		t.Errorf("got out=%q truncated=%v total=%d", out, truncated, total)
	}
}

func TestTailLines(t *testing.T) {
	got := tailLines("1\n2\n3\n4\n5\n6\n", 3)
	if got != "4\n5\n6" {
		t.Errorf("tailLines = %q, want %q", got, "4\n5\n6")
	}
	if got := tailLines("", 3); got != "" {
		t.Errorf("tailLines(empty) = %q", got)
	}
	if got := tailLines("only", 3); got != "only" {
		t.Errorf("tailLines(short) = %q", got)
	}
}

func TestParseHostSpecErrorMessage(t *testing.T) {
	_, err := parseHostSpec("bad")
	if err == nil || !strings.Contains(err.Error(), "want name=user@host") {
		t.Errorf("unexpected error: %v", err)
	}
}
