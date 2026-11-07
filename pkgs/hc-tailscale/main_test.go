package main

import (
	"bytes"
	"context"
	"io"
	"log"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

type fakeNode struct {
	slot     string
	hostname string
	readyErr error
	target   string // addr dials are redirected to
	mu       sync.Mutex
	dialed   []string
}

func (f *fakeNode) ready(context.Context) error { return f.readyErr }
func (f *fakeNode) dial(ctx context.Context, addr string) (net.Conn, error) {
	f.mu.Lock()
	f.dialed = append(f.dialed, addr)
	f.mu.Unlock()
	var d net.Dialer
	return d.DialContext(ctx, "tcp", f.target)
}
func (f *fakeNode) status(context.Context) (slotStatus, error) {
	return slotStatus{Slot: f.slot, Hostname: f.hostname, State: "Running", Tailnet: "example.ts.net"}, nil
}
func (f *fakeNode) login(_ context.Context, emit func(string)) (slotStatus, error) {
	emit("https://login.example/a/1")
	return slotStatus{Slot: f.slot, Hostname: f.hostname, State: "Running"}, nil
}
func (f *fakeNode) logout(context.Context) error { return nil }
func (f *fakeNode) close() error                 { return nil }

// echoServer upper-cases what it reads, then half-closes once the client has.
func echoServer(t *testing.T) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			go func(c net.Conn) {
				defer c.Close()
				b, _ := io.ReadAll(c)
				_, _ = c.Write(bytes.ToUpper(b))
			}(c)
		}
	}()
	return ln.Addr().String()
}

func startDaemon(t *testing.T, mk func(slot, dir, host string) (node, error)) (*daemon, string) {
	t.Helper()
	dir := t.TempDir()
	sock := filepath.Join(dir, "s.sock")
	ln, err := listen(sock)
	if err != nil {
		t.Fatal(err)
	}
	d := newDaemon(filepath.Join(dir, "state"), log.New(io.Discard, "", 0))
	d.newNode = mk
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	go d.serve(ctx, ln)
	t.Setenv("HC_TAILSCALE_SOCKET", sock)
	return d, dir
}

func TestValidSlot(t *testing.T) {
	for _, ok := range []string{"default", "work", "a", "tail-2"} {
		if err := validSlot(ok); err != nil {
			t.Errorf("%q rejected: %v", ok, err)
		}
	}
	for _, bad := range []string{"", "Work", "../x", "a/b", "-x", "a b", strings.Repeat("a", 64)} {
		if validSlot(bad) == nil {
			t.Errorf("%q accepted", bad)
		}
	}
}

func TestParseTarget(t *testing.T) {
	t.Setenv("USER", "me")
	cases := map[string][2]string{
		"romeo":              {"me", "romeo:22"},
		"bob@romeo":          {"bob", "romeo:22"},
		"bob@romeo:2222":     {"bob", "romeo:2222"},
		"100.64.0.1":         {"me", "100.64.0.1:22"},
		"bob@[fd7a::1]":      {"bob", "[fd7a::1]:22"},
		"bob@[fd7a::1]:2200": {"bob", "[fd7a::1]:2200"},
	}
	for in, want := range cases {
		u, a, err := parseTarget(in)
		if err != nil || u != want[0] || a != want[1] {
			t.Errorf("parseTarget(%q) = %q, %q, %v; want %q, %q", in, u, a, err, want[0], want[1])
		}
	}
	if _, _, err := parseTarget("bob@"); err == nil {
		t.Error("empty host accepted")
	}
}

func TestRemoteCommandIsVerbatim(t *testing.T) {
	line := `exec sh -c 'hc connect --session "x y"'`
	if got := remoteCommand([]string{line}); got != line {
		t.Errorf("single arg changed: %q", got)
	}
	if got := remoteCommand([]string{"echo", "a", "b"}); got != "echo a b" {
		t.Errorf("join: %q", got)
	}
}

func TestCheckBanner(t *testing.T) {
	if !isCheckBanner("# Tailscale SSH requires an additional check.\n# To authenticate, visit: https://login.tailscale.com/a/abc") {
		t.Error("check banner missed")
	}
	if isCheckBanner("Welcome to romeo") {
		t.Error("plain banner flagged")
	}
}

func TestReadLineDoesNotReadAhead(t *testing.T) {
	r := strings.NewReader("{\"ok\":true}\nSSH-2.0-rest")
	if _, err := readLine(r); err != nil {
		t.Fatal(err)
	}
	rest, _ := io.ReadAll(r)
	if string(rest) != "SSH-2.0-rest" {
		t.Errorf("stream after the line = %q", rest)
	}
}

func TestDialSplicesBothWays(t *testing.T) {
	target := echoServer(t)
	var fn *fakeNode
	startDaemon(t, func(slot, _, host string) (node, error) {
		fn = &fakeNode{slot: slot, hostname: host, target: target}
		return fn, nil
	})

	// A slot only exists once it has logged in.
	if err := runLogin("work", "", io.Discard); err != nil {
		t.Fatal(err)
	}

	c, err := connectDaemon()
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	r, err := roundTrip(c, request{Op: "dial", Slot: "work", Addr: "romeo:22"})
	if err != nil || !r.OK {
		t.Fatalf("dial: %+v %v", r, err)
	}
	payload := []byte("binary\x00\xff\nbytes")
	if _, err := c.Write(payload); err != nil {
		t.Fatal(err)
	}
	_ = c.(*net.UnixConn).CloseWrite()
	_ = c.SetReadDeadline(time.Now().Add(5 * time.Second))
	got, err := io.ReadAll(c)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, bytes.ToUpper(payload)) {
		t.Errorf("got %q", got)
	}
	if len(fn.dialed) != 1 || fn.dialed[0] != "romeo:22" {
		t.Errorf("dialed %v", fn.dialed)
	}
}

func TestDialUnknownSlotNeedsLogin(t *testing.T) {
	created := false
	d, _ := startDaemon(t, func(string, string, string) (node, error) {
		created = true
		return &fakeNode{}, nil
	})
	c, err := connectDaemon()
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	r, err := roundTrip(c, request{Op: "dial", Slot: "typo", Addr: "romeo:22"})
	if err != nil {
		t.Fatal(err)
	}
	if r.OK || r.Code != codeNeedsLogin {
		t.Errorf("reply %+v", r)
	}
	if created {
		t.Error("a dial registered a new node")
	}
	if _, err := os.Stat(d.slotDir("typo")); err == nil {
		t.Error("a dial created state on disk")
	}
	if !strings.Contains(replyErr("typo", r).Error(), "hc-tailscale --slot typo login") {
		t.Errorf("hint: %v", replyErr("typo", r))
	}
}

func TestDialNotReadyFailsFast(t *testing.T) {
	startDaemon(t, func(slot, _, _ string) (node, error) {
		return &fakeNode{slot: slot, readyErr: &codedError{codeNeedsLogin, "slot \"default\" is not logged in"}}, nil
	})
	if err := runLogin("default", "", io.Discard); err != nil {
		t.Fatal(err)
	}
	err := runSSH("default", "bob@romeo", []string{"true"})
	ee, ok := err.(*exitError)
	if !ok || ee.code != exitTransport || !strings.Contains(ee.msg, "login") {
		t.Errorf("runSSH = %v", err)
	}
}

func TestLoginHostnamePersistsAndSlotsList(t *testing.T) {
	hosts := map[string]string{}
	d, _ := startDaemon(t, func(slot, _, host string) (node, error) {
		hosts[slot] = host
		return &fakeNode{slot: slot, hostname: host}, nil
	})
	var out bytes.Buffer
	if err := runLogin("work", "laptop-work", &out); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "https://login.example/a/1") {
		t.Errorf("login output %q", out.String())
	}
	if err := runLogin("default", "", io.Discard); err != nil {
		t.Fatal(err)
	}
	if hosts["work"] != "laptop-work" || !strings.HasSuffix(hosts["default"], "-hc") {
		t.Errorf("hostnames %v", hosts)
	}
	if got := d.hostnameFor("work"); got != "laptop-work" {
		t.Errorf("persisted hostname %q", got)
	}
	out.Reset()
	if err := runSimple("status", "", &out); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "default\tRunning") || !strings.Contains(out.String(), "work\tRunning") {
		t.Errorf("slots output %q", out.String())
	}
}

func TestDefaultHostname(t *testing.T) {
	if h := defaultHostname("default"); !strings.HasSuffix(h, "-hc") || strings.Contains(h, ".") {
		t.Errorf("default: %q", h)
	}
	if h := defaultHostname("work"); !strings.HasSuffix(h, "-hc-work") {
		t.Errorf("work: %q", h)
	}
}

func TestListenRefusesLiveSocket(t *testing.T) {
	sock := filepath.Join(t.TempDir(), "s.sock")
	ln, err := listen(sock)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			c.Close()
		}
	}()
	if _, err := listen(sock); err == nil {
		t.Error("second daemon took over a live socket")
	}
	fi, err := os.Stat(sock)
	if err != nil {
		t.Fatal(err)
	}
	if fi.Mode().Perm() != 0o600 {
		t.Errorf("socket mode %v", fi.Mode().Perm())
	}
}
