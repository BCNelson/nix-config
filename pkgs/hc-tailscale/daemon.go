package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

// daemon owns every slot's tsnet node. One process per user, so two hc
// connections through the same slot share one node key instead of fighting
// over it, and only the first one pays for coming up on the tailnet.
type daemon struct {
	base    string
	logger  *log.Logger
	newNode func(slot, dir, hostname string) (node, error)

	mu    sync.Mutex
	slots map[string]node
	// starting serialises node creation per slot without holding mu across
	// a tsnet start.
	starting map[string]*sync.Mutex
}

func newDaemon(base string, logger *log.Logger) *daemon {
	d := &daemon{
		base:     base,
		logger:   logger,
		slots:    map[string]node{},
		starting: map[string]*sync.Mutex{},
	}
	d.newNode = func(slot, dir, hostname string) (node, error) {
		return newTSNode(slot, dir, hostname, logger)
	}
	return d
}

func (d *daemon) slotDir(slot string) string { return filepath.Join(d.base, slot) }

// defaultHostname names the tsnet device: "<machine>-hc" for the default slot,
// "<machine>-hc-<slot>" otherwise, so several slots on one tailnet stay apart
// in the admin console.
func defaultHostname(slot string) string {
	h, err := os.Hostname()
	if err != nil || h == "" {
		h = "localhost"
	}
	h, _, _ = strings.Cut(h, ".")
	if slot == defaultSlot {
		return h + "-hc"
	}
	return h + "-hc-" + slot
}

// hostnameFor reads the hostname a slot was created with, so a --hostname
// given at first login survives daemon restarts.
func (d *daemon) hostnameFor(slot string) string {
	b, err := os.ReadFile(filepath.Join(d.slotDir(slot), "hostname"))
	if err == nil {
		if h := strings.TrimSpace(string(b)); h != "" {
			return h
		}
	}
	return defaultHostname(slot)
}

// knownSlots are the slots with state on disk.
func (d *daemon) knownSlots() []string {
	ents, err := os.ReadDir(d.base)
	if err != nil {
		return nil
	}
	var out []string
	for _, e := range ents {
		if e.IsDir() && validSlot(e.Name()) == nil {
			out = append(out, e.Name())
		}
	}
	return out
}

// get returns the slot's node, starting it if needed. With create false a slot
// that has no state on disk is an error rather than a new tailnet device:
// dialing through a typo'd slot should not register a node.
func (d *daemon) get(slot, hostname string, create bool) (node, error) {
	if err := validSlot(slot); err != nil {
		return nil, &codedError{codeBadRequest, err.Error()}
	}
	d.mu.Lock()
	if n, ok := d.slots[slot]; ok {
		d.mu.Unlock()
		return n, nil
	}
	sm, ok := d.starting[slot]
	if !ok {
		sm = &sync.Mutex{}
		d.starting[slot] = sm
	}
	d.mu.Unlock()

	sm.Lock()
	defer sm.Unlock()
	d.mu.Lock()
	if n, ok := d.slots[slot]; ok {
		d.mu.Unlock()
		return n, nil
	}
	d.mu.Unlock()

	dir := d.slotDir(slot)
	if _, err := os.Stat(dir); errors.Is(err, os.ErrNotExist) {
		if !create {
			return nil, &codedError{codeNeedsLogin, fmt.Sprintf("slot %q is not logged in", slot)}
		}
		if err := os.MkdirAll(dir, 0o700); err != nil {
			return nil, err
		}
		if hostname != "" {
			if err := os.WriteFile(filepath.Join(dir, "hostname"), []byte(hostname+"\n"), 0o600); err != nil {
				return nil, err
			}
		}
	}

	n, err := d.newNode(slot, dir, d.hostnameFor(slot))
	if err != nil {
		return nil, err
	}
	d.mu.Lock()
	d.slots[slot] = n
	d.mu.Unlock()
	return n, nil
}

// startKnown brings up every slot already on disk, in the background, so the
// first hc connection after boot does not wait for control.
func (d *daemon) startKnown() {
	for _, s := range d.knownSlots() {
		go func(s string) {
			if _, err := d.get(s, "", false); err != nil {
				d.logger.Printf("[%s] start: %v", s, err)
			}
		}(s)
	}
}

func (d *daemon) serve(ctx context.Context, ln net.Listener) error {
	go func() {
		<-ctx.Done()
		_ = ln.Close()
	}()
	for {
		c, err := ln.Accept()
		if err != nil {
			if ctx.Err() != nil {
				return nil
			}
			return err
		}
		go d.handle(ctx, c)
	}
}

func fail(c net.Conn, err error) {
	code := codeInternal
	var ce *codedError
	if errors.As(err, &ce) {
		code = ce.code
	}
	_ = writeJSONLine(c, reply{Code: code, Error: err.Error(), Done: true})
}

func (d *daemon) handle(ctx context.Context, c net.Conn) {
	// A dial hands c off to the splice, which closes it.
	handedOff := false
	defer func() {
		if !handedOff {
			_ = c.Close()
		}
	}()

	_ = c.SetReadDeadline(time.Now().Add(10 * time.Second))
	var req request
	if err := readJSONLine(c, &req); err != nil {
		fail(c, &codedError{codeBadRequest, "bad request: " + err.Error()})
		return
	}
	_ = c.SetReadDeadline(time.Time{})

	// Stop work for a client that went away (hc killed its --via command).
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()

	switch req.Op {
	case "dial":
		if req.Addr == "" {
			fail(c, &codedError{codeBadRequest, "dial needs addr"})
			return
		}
		n, err := d.get(req.Slot, "", false)
		if err != nil {
			fail(c, err)
			return
		}
		if err := n.ready(ctx); err != nil {
			fail(c, err)
			return
		}
		dctx, dcancel := context.WithTimeout(ctx, 20*time.Second)
		up, err := n.dial(dctx, req.Addr)
		dcancel()
		if err != nil {
			fail(c, &codedError{codeDial, fmt.Sprintf("dial %s via slot %q: %v", req.Addr, req.Slot, err)})
			return
		}
		if err := writeJSONLine(c, reply{OK: true, Done: true}); err != nil {
			_ = up.Close()
			return
		}
		handedOff = true
		go splice(c, up)

	case "login":
		n, err := d.get(req.Slot, req.Hostname, true)
		if err != nil {
			fail(c, err)
			return
		}
		// The client never writes again; a read returning means it hung up.
		go func() {
			_, _ = io.Copy(io.Discard, c)
			cancel()
		}()
		st, err := n.login(ctx, func(url string) {
			_ = writeJSONLine(c, reply{OK: true, AuthURL: url})
		})
		if err != nil {
			fail(c, err)
			return
		}
		_ = writeJSONLine(c, reply{OK: true, Done: true, Slots: []slotStatus{st}})

	case "status":
		slots := []string{req.Slot}
		if req.Slot == "" {
			slots = d.knownSlots()
		}
		var out []slotStatus
		for _, s := range slots {
			n, err := d.get(s, "", false)
			if err != nil {
				out = append(out, slotStatus{Slot: s, State: "NotLoggedIn"})
				continue
			}
			st, err := n.status(ctx)
			if err != nil {
				out = append(out, slotStatus{Slot: s, State: "Error: " + err.Error()})
				continue
			}
			out = append(out, st)
		}
		sort.Slice(out, func(i, j int) bool { return out[i].Slot < out[j].Slot })
		_ = writeJSONLine(c, reply{OK: true, Done: true, Slots: out})

	case "logout":
		n, err := d.get(req.Slot, "", false)
		if err != nil {
			fail(c, err)
			return
		}
		if err := n.logout(ctx); err != nil {
			fail(c, err)
			return
		}
		_ = writeJSONLine(c, reply{OK: true, Done: true})

	default:
		fail(c, &codedError{codeBadRequest, fmt.Sprintf("unknown op %q", req.Op)})
	}
}

// splice copies both ways and half-closes, so an EOF from one side reaches the
// other as an EOF rather than a reset -- ssh relies on that to finish cleanly.
func splice(a, b net.Conn) {
	var wg sync.WaitGroup
	wg.Add(2)
	cp := func(dst, src net.Conn) {
		defer wg.Done()
		_, _ = io.Copy(dst, src)
		if cw, ok := dst.(interface{ CloseWrite() error }); ok {
			_ = cw.CloseWrite()
		} else {
			_ = dst.Close()
		}
	}
	go cp(a, b)
	go cp(b, a)
	wg.Wait()
	_ = a.Close()
	_ = b.Close()
}

func (d *daemon) closeAll() {
	d.mu.Lock()
	defer d.mu.Unlock()
	for s, n := range d.slots {
		if err := n.close(); err != nil {
			d.logger.Printf("[%s] close: %v", s, err)
		}
	}
}

// listen opens the socket, replacing a stale one from a crashed daemon but not
// stealing it from a live one.
func listen(path string) (net.Listener, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, err
	}
	if c, err := net.DialTimeout("unix", path, time.Second); err == nil {
		_ = c.Close()
		return nil, fmt.Errorf("another hc-tailscale daemon is already listening on %s", path)
	}
	_ = os.Remove(path)
	ln, err := net.Listen("unix", path)
	if err != nil {
		return nil, err
	}
	// Anyone who can connect can dial into the tailnet as this node.
	if err := os.Chmod(path, 0o600); err != nil {
		_ = ln.Close()
		return nil, err
	}
	return ln, nil
}
