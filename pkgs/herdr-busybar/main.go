// herdr-busybar feeds a BUSY Bar the number of coding agents in each status,
// as herdr-core (`hc`) sees them.
//
// The bar is plugged in over USB, where it shows up as a point-to-point
// network: the bar is 10.0.4.20, this host 10.0.4.21. A JavaScript app on the
// bar (./app) polls GET /v1/counts here once a second and draws the result.
// Pulling from the bar rather than pushing draws from here keeps the screen
// owned by the app, so the bar's own menus and BUSY sessions behave normally.
//
// The bridge also keeps that app installed: whenever the bar (re)appears, or
// reboots, it checks the installed version against the package it was built
// with, installs it if they differ, and launches it.
package main

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"
)

func main() {
	listen := flag.String("listen", "10.0.4.21:8787", "address the bar app polls (this host's end of the USB link)")
	barURL := flag.String("bar", "http://10.0.4.20", "the bar's HTTP API")
	sock := flag.String("core-socket", defaultSocket(), "herdr-core's socket")
	appPkg := flag.String("app", "", "app package (.tgz) to keep installed on the bar; empty to leave the bar alone")
	launch := flag.Bool("launch", true, "launch the app when the bar appears or reboots")
	idleAsDone := flag.Duration("idle-as-done", 30*time.Minute, "how long an idle agent still counts as done, from when it finished")
	view := flag.String("app-view", defaultAppView(), "the herdr app's view.json, for which done agents you have already seen")
	flag.Parse()

	h := &core{socket: *sock, idleAsDone: *idleAsDone, view: *view}
	http.HandleFunc("/v1/counts", func(w http.ResponseWriter, r *http.Request) {
		c, err := h.counts()
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("Cache-Control", "no-store")
		if err != nil {
			// Still 200: the app treats ok=false as "no core running",
			// which is a normal state, not a transport error.
			c = counts{Error: err.Error()}
		} else {
			c.OK = true
		}
		_ = json.NewEncoder(w).Encode(c)
	})

	if *appPkg != "" {
		b, err := newBar(*barURL, *appPkg, *launch)
		if err != nil {
			log.Fatalf("app package: %v", err)
		}
		go b.keepInstalled()
	}

	ln, err := listenFreebind(*listen)
	if err != nil {
		log.Fatalf("listen %s: %v", *listen, err)
	}
	log.Printf("serving %s (herdr-core socket %s)", *listen, *sock)
	srv := &http.Server{ReadHeaderTimeout: 5 * time.Second, WriteTimeout: 10 * time.Second, IdleTimeout: 60 * time.Second}
	log.Fatal(srv.Serve(ln))
}

// listenFreebind binds even while the address is missing. 10.0.4.21 only
// exists while the bar is plugged in (the bar's DHCP server hands it out), so
// a plain bind would fail on an undocked laptop and need restarting on every
// dock. Binding the one address rather than 0.0.0.0 keeps the port off Wi-Fi:
// Fedora Workstation's firewall leaves 1025-65535 open on every interface.
func listenFreebind(addr string) (net.Listener, error) {
	lc := net.ListenConfig{Control: func(_, _ string, c syscall.RawConn) error {
		var serr error
		if err := c.Control(func(fd uintptr) {
			serr = syscall.SetsockoptInt(int(fd), syscall.IPPROTO_IP, syscall.IP_FREEBIND, 1)
		}); err != nil {
			return err
		}
		return serr
	}}
	return lc.Listen(context.Background(), "tcp4", addr)
}

// defaultSocket is where herdr-core listens for session HC_SESSION
// (default "default"): $XDG_RUNTIME_DIR/herdr-core/<session>.sock.
func defaultSocket() string {
	session := os.Getenv("HC_SESSION")
	if session == "" {
		session = "default"
	}
	run := os.Getenv("XDG_RUNTIME_DIR")
	if run == "" {
		run = fmt.Sprintf("/run/user/%d", os.Getuid())
	}
	return filepath.Join(run, "herdr-core", session+".sock")
}

func defaultAppView() string {
	cfg := os.Getenv("XDG_CONFIG_HOME")
	if cfg == "" {
		home, _ := os.UserHomeDir()
		cfg = filepath.Join(home, ".config")
	}
	return filepath.Join(cfg, "herdr-app", "view.json")
}

// seenDone reads the herdr app's record of which terminals' attention you
// have already looked at. herdr-core says "done" from the end of a turn until
// the next one; the app shows that as done only "while you weren't looking"
// and as idle once you look (its herdr.d.ts), and it keeps the looking in
// view.json's "attention" map: address -> {word, seen}. A terminal counts as
// seen-done when the app saw it with the word "done".
//
// No app, or an unreadable file, just means nothing is known to be seen, and
// herdr-core's word stands.
func seenDone(path string) map[string]bool {
	raw, err := os.ReadFile(path)
	if err != nil {
		return nil
	}
	var v struct {
		Attention map[string]struct {
			Word *string `json:"word"`
			Seen bool    `json:"seen"`
		} `json:"attention"`
	}
	if json.Unmarshal(raw, &v) != nil {
		return nil
	}
	seen := make(map[string]bool, len(v.Attention))
	for addr, a := range v.Attention {
		if a.Seen && a.Word != nil && *a.Word == "done" {
			seen[addr] = true
		}
	}
	return seen
}

// ----------------------------------------------------------- herdr-core ---

// The bar shows blocked, working and done. Idle holds the agents it does not
// show (idle for longer than -idle-as-done), so the four add up to every agent.
type counts struct {
	OK      bool   `json:"ok"`
	Blocked int    `json:"blocked"`
	Working int    `json:"working"`
	Done    int    `json:"done"`
	Idle    int    `json:"idle"`
	Error   string `json:"error,omitempty"`
}

// core reads herdr-core's socket directly instead of running `hc ls`: any hc
// command starts a core when none is running, handing it the caller's
// environment -- which every terminal it then opens would inherit. A display
// must never be the thing that starts the core, so a missing socket is just
// "nothing to show".
type core struct {
	socket string

	// An agent that has gone idle counts towards done for this long after
	// it went idle, then is not shown at all. ("done" itself always counts.)
	idleAsDone time.Duration

	// The herdr app's view.json; see seenDone.
	view string

	// The bar polls once a second; a short cache keeps a second client (or a
	// curl in a terminal) from doubling the socket traffic.
	mu   sync.Mutex
	at   time.Time
	last counts
	err  error
}

func (h *core) counts() (counts, error) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if time.Since(h.at) < 500*time.Millisecond {
		return h.last, h.err
	}
	h.last, h.err = h.fetch()
	h.at = time.Now()
	return h.last, h.err
}

func (h *core) fetch() (counts, error) {
	var res struct {
		Terminals []struct {
			// The agent's own status record (OSC 7501); null for a plain
			// shell, or once the program has exited.
			Status *struct {
				State string `json:"state"`
			} `json:"status"`
			// When Status last changed, Unix milliseconds.
			StatusMs int64  `json:"status_ms"`
			Address  string `json:"address"`
		} `json:"terminals"`
	}
	// Live terminals only (all=false), so finished agents drop off.
	if err := h.call("terminal.list", map[string]any{}, &res); err != nil {
		return counts{}, err
	}
	var c counts
	now := time.Now()
	seen := seenDone(h.view)
	for _, t := range res.Terminals {
		if t.Status == nil {
			continue // not an agent
		}
		state := t.Status.State
		if state == "done" && seen[t.Address] {
			// What the app shows: done you have looked at is idle. Its 30
			// minutes run from when it finished (status_ms), as herdr-core
			// does not say when you looked.
			state = "idle"
		}
		// herdr-core's words: idle, working, done, blocked, error.
		switch state {
		case "blocked", "error":
			c.Blocked++ // both want you
		case "working":
			c.Working++
		case "done":
			c.Done++
		case "idle":
			if now.Sub(time.UnixMilli(t.StatusMs)) <= h.idleAsDone {
				c.Done++ // recently went idle: still news
			} else {
				c.Idle++
			}
		default:
			c.Idle++ // a word herdr-core adds later
		}
	}
	return c, nil
}

// call makes one JSON-RPC 2.0 request on the core's socket, one message per
// line. A fresh connection each time: a list is ~6 KB and well under a
// millisecond, and there is then no connection to notice going stale when
// the core restarts.
func (h *core) call(method string, params, out any) error {
	conn, err := net.DialTimeout("unix", h.socket, 2*time.Second)
	if err != nil {
		return fmt.Errorf("herdr-core not running: %w", err)
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(5 * time.Second))

	req, _ := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
	if _, err := conn.Write(append(req, '\n')); err != nil {
		return err
	}
	var resp struct {
		Result json.RawMessage `json:"result"`
		Error  *struct {
			Code    int    `json:"code"`
			Message string `json:"message"`
		} `json:"error"`
	}
	if err := json.NewDecoder(conn).Decode(&resp); err != nil {
		return fmt.Errorf("%s: %w", method, err)
	}
	if resp.Error != nil {
		return fmt.Errorf("%s: %d: %s", method, resp.Error.Code, resp.Error.Message)
	}
	return json.Unmarshal(resp.Result, out)
}

// ------------------------------------------------------------------ bar ---

type bar struct {
	base    string
	pkg     []byte
	id      string
	version string
	launch  bool
	client  *http.Client
}

func newBar(base, pkgPath string, launch bool) (*bar, error) {
	pkg, err := os.ReadFile(pkgPath)
	if err != nil {
		return nil, err
	}
	m, err := manifestOf(pkg)
	if err != nil {
		return nil, err
	}
	return &bar{
		base: strings.TrimRight(base, "/"), pkg: pkg, id: m.ID, version: m.Version, launch: launch,
		client: &http.Client{Timeout: 30 * time.Second},
	}, nil
}

type manifest struct {
	ID      string `json:"id"`
	Version string `json:"version"`
}

func manifestOf(tgz []byte) (manifest, error) {
	zr, err := gzip.NewReader(bytes.NewReader(tgz))
	if err != nil {
		return manifest{}, err
	}
	tr := tar.NewReader(zr)
	for {
		hdr, err := tr.Next()
		if err != nil {
			return manifest{}, errors.New("no <id>/appmeta/manifest.json in the package")
		}
		if strings.HasSuffix(hdr.Name, "/appmeta/manifest.json") {
			var m manifest
			if err := json.NewDecoder(tr).Decode(&m); err != nil {
				return manifest{}, err
			}
			return m, nil
		}
	}
}

// keepInstalled watches for the bar, and for each new bar session (it was
// plugged in, or it rebooted -- which also quits apps) makes sure the app is
// current and running. Between sessions it only checks the boot time, so a
// docked laptop costs one tiny request every few seconds.
func (b *bar) keepInstalled() {
	var session int64 = -1
	for ; ; time.Sleep(5 * time.Second) {
		boot, err := b.bootTime()
		if err != nil {
			session = -1 // unplugged; the next sighting is a new session
			continue
		}
		if boot == session {
			continue
		}
		if err := b.sync(); err != nil {
			log.Printf("bar: %v", err)
			continue // retried next tick, still as a new session
		}
		session = boot
	}
}

func (b *bar) bootTime() (int64, error) {
	var st struct {
		System struct {
			BootTime int64 `json:"boot_time"`
		} `json:"system"`
	}
	c := &http.Client{Timeout: 3 * time.Second}
	resp, err := c.Get(b.base + "/api/status")
	if err != nil {
		return 0, err
	}
	defer resp.Body.Close()
	if err := json.NewDecoder(resp.Body).Decode(&st); err != nil {
		return 0, err
	}
	return st.System.BootTime, nil
}

func (b *bar) sync() error {
	installed, err := b.installedVersion()
	if err != nil {
		return err
	}
	if installed != b.version {
		log.Printf("bar: installing %s %s (bar has %q)", b.id, b.version, installed)
		if err := b.install(); err != nil {
			return fmt.Errorf("install: %w", err)
		}
	}
	if !b.launch {
		return nil
	}
	resp, err := b.client.Post(b.base+"/api/apps/launch?app_id="+url.QueryEscape(b.id), "", nil)
	if err != nil {
		return fmt.Errorf("launch: %w", err)
	}
	body, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	switch resp.StatusCode {
	case http.StatusOK:
		log.Printf("bar: launched %s", b.id)
	case http.StatusConflict:
		// A BUSY session (or another higher-priority app) owns the screen.
		// Leave it be; the app is in the APPS menu for later.
		log.Printf("bar: not launching, the bar is busy: %s", bytes.TrimSpace(body))
	default:
		return fmt.Errorf("launch: HTTP %d: %s", resp.StatusCode, bytes.TrimSpace(body))
	}
	return nil
}

func (b *bar) installedVersion() (string, error) {
	resp, err := b.client.Get(b.base + "/api/apps/list")
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	var list struct {
		Apps []manifest `json:"apps"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&list); err != nil {
		return "", fmt.Errorf("apps/list: %w", err)
	}
	for _, a := range list.Apps {
		if a.ID == b.id {
			return a.Version, nil
		}
	}
	return "", nil
}

// install is the bar's two-step installer: stage the package, then confirm
// with the key the stage returns (what busy-tools' `app install` does).
func (b *bar) install() error {
	resp, err := b.client.Post(b.base+"/api/apps/stage", "application/octet-stream", bytes.NewReader(b.pkg))
	if err != nil {
		return err
	}
	body, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("stage: HTTP %d: %s", resp.StatusCode, bytes.TrimSpace(body))
	}
	var staged struct {
		InstallKey int64 `json:"install_key"`
	}
	if err := json.Unmarshal(body, &staged); err != nil {
		return fmt.Errorf("stage: %w", err)
	}
	resp, err = b.client.Post(fmt.Sprintf("%s/api/apps/install?install_key=%d", b.base, staged.InstallKey), "", nil)
	if err != nil {
		return err
	}
	body, _ = io.ReadAll(resp.Body)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("install: HTTP %d: %s", resp.StatusCode, bytes.TrimSpace(body))
	}
	return nil
}
