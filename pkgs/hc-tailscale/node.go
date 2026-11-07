package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"net"
	"time"

	"tailscale.com/client/local"
	"tailscale.com/ipn"
	"tailscale.com/tsnet"
)

// node is one slot's tailnet presence. It is an interface so the daemon can be
// tested without a control server.
type node interface {
	// ready returns nil once the node can dial, a *codedError with
	// codeNeedsLogin/codeMachineAuth when a human has to act first, or another
	// error. It never waits on a human.
	ready(ctx context.Context) error
	dial(ctx context.Context, addr string) (net.Conn, error)
	status(ctx context.Context) (slotStatus, error)
	// login starts an interactive login and calls emit with every new auth URL
	// until the node is running or ctx ends.
	login(ctx context.Context, emit func(url string)) (slotStatus, error)
	logout(ctx context.Context) error
	close() error
}

type codedError struct {
	code string
	msg  string
}

func (e *codedError) Error() string { return e.msg }

type tsNode struct {
	slot     string
	hostname string
	srv      *tsnet.Server
	lc       *local.Client
}

func newTSNode(slot, dir, hostname string, logger *log.Logger) (*tsNode, error) {
	srv := &tsnet.Server{
		Dir:      dir,
		Hostname: hostname,
		// The backend's chatter goes to the journal, prefixed by slot. The
		// "user" log is only the auth-URL reminder, which login hands to the
		// person who asked for it instead.
		Logf:     func(f string, a ...any) { logger.Printf("["+slot+"] "+f, a...) },
		UserLogf: func(string, ...any) {},
	}
	if err := srv.Start(); err != nil {
		return nil, fmt.Errorf("start slot %s: %w", slot, err)
	}
	lc, err := srv.LocalClient()
	if err != nil {
		_ = srv.Close()
		return nil, err
	}
	return &tsNode{slot: slot, hostname: hostname, srv: srv, lc: lc}, nil
}

// readyTimeout bounds how long a dial waits for a slot that is still starting
// (a fresh daemon reconnecting to control). Long enough for a slow DERP
// handshake, short enough that hc sees a clear error rather than a hang.
const readyTimeout = 20 * time.Second

func (n *tsNode) ready(ctx context.Context) error {
	ctx, cancel := context.WithTimeout(ctx, readyTimeout)
	defer cancel()
	for {
		st, err := n.lc.StatusWithoutPeers(ctx)
		if err != nil {
			return err
		}
		switch st.BackendState {
		case ipn.Running.String():
			return nil
		case ipn.NeedsMachineAuth.String():
			return &codedError{codeMachineAuth, fmt.Sprintf("slot %q is waiting for an admin to approve this device", n.slot)}
		case ipn.NeedsLogin.String():
			// tsnet passes through NeedsLogin briefly even with a saved key,
			// so only an auth URL from control proves a human is needed.
			if st.AuthURL != "" {
				return &codedError{codeNeedsLogin, fmt.Sprintf("slot %q is not logged in", n.slot)}
			}
		case ipn.Stopped.String():
			return fmt.Errorf("slot %q is stopped", n.slot)
		}
		select {
		case <-ctx.Done():
			return fmt.Errorf("slot %q did not come up within %s (state %s)", n.slot, readyTimeout, st.BackendState)
		case <-time.After(200 * time.Millisecond):
		}
	}
}

func (n *tsNode) dial(ctx context.Context, addr string) (net.Conn, error) {
	return n.srv.Dial(ctx, "tcp", addr)
}

func (n *tsNode) status(ctx context.Context) (slotStatus, error) {
	st, err := n.lc.StatusWithoutPeers(ctx)
	if err != nil {
		return slotStatus{}, err
	}
	out := slotStatus{Slot: n.slot, Hostname: n.hostname, State: st.BackendState, AuthURL: st.AuthURL}
	if st.CurrentTailnet != nil {
		out.Tailnet = st.CurrentTailnet.Name
	}
	if st.Self != nil {
		out.DNSName = st.Self.DNSName
	}
	for _, ip := range st.TailscaleIPs {
		out.IPs = append(out.IPs, ip.String())
	}
	return out, nil
}

func (n *tsNode) login(ctx context.Context, emit func(string)) (slotStatus, error) {
	st, err := n.lc.StatusWithoutPeers(ctx)
	if err != nil {
		return slotStatus{}, err
	}
	if st.BackendState == ipn.Running.String() {
		return n.status(ctx)
	}

	w, err := n.lc.WatchIPNBus(ctx, ipn.NotifyInitialState)
	if err != nil {
		return slotStatus{}, err
	}
	defer w.Close()

	// tsnet already asked control for a URL at start if it needed one; ask
	// again only when there is none yet, or each login would mint a new one.
	if st.AuthURL == "" {
		if err := n.lc.StartLoginInteractive(ctx); err != nil {
			return slotStatus{}, err
		}
	}

	var last string
	if st.AuthURL != "" {
		last = st.AuthURL
		emit(last)
	}
	for {
		nt, err := w.Next()
		if err != nil {
			return slotStatus{}, err
		}
		if nt.ErrMessage != nil {
			return slotStatus{}, errors.New(*nt.ErrMessage)
		}
		if nt.BrowseToURL != nil && *nt.BrowseToURL != "" && *nt.BrowseToURL != last {
			last = *nt.BrowseToURL
			emit(last)
		}
		if nt.State != nil && *nt.State == ipn.Running {
			// Running can precede the netmap that carries our IPs.
			for range 50 {
				s, err := n.status(ctx)
				if err != nil || len(s.IPs) > 0 {
					return s, err
				}
				time.Sleep(100 * time.Millisecond)
			}
			return n.status(ctx)
		}
	}
}

func (n *tsNode) logout(ctx context.Context) error { return n.lc.Logout(ctx) }

func (n *tsNode) close() error { return n.srv.Close() }
