package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"os/user"
	"strings"
	"time"

	"golang.org/x/crypto/ssh"
)

// exitTransport is what ssh(1) exits with when the connection itself failed,
// which hc already reads as "could not reach the machine".
const exitTransport = 255

type exitError struct {
	code int
	msg  string
}

func (e *exitError) Error() string { return e.msg }

func connectDaemon() (net.Conn, error) {
	p, err := socketPath()
	if err != nil {
		return nil, err
	}
	c, err := net.DialTimeout("unix", p, 5*time.Second)
	if err != nil {
		return nil, fmt.Errorf("the hc-tailscale daemon is not running (%v); start it with: systemctl --user start hc-tailscale", err)
	}
	return c, nil
}

// roundTrip sends one request and returns the daemon's first reply.
func roundTrip(c net.Conn, req request) (reply, error) {
	if err := writeJSONLine(c, req); err != nil {
		return reply{}, err
	}
	var r reply
	if err := readJSONLine(c, &r); err != nil {
		return reply{}, fmt.Errorf("daemon: %w", err)
	}
	return r, nil
}

func replyErr(slot string, r reply) error {
	switch r.Code {
	case codeNeedsLogin:
		return fmt.Errorf("%s; run: hc-tailscale --slot %s login", r.Error, slot)
	case codeMachineAuth:
		return fmt.Errorf("%s in the Tailscale admin console", r.Error)
	}
	return errors.New(r.Error)
}

// parseTarget splits [user@]host[:port]; the user defaults to the local one,
// like ssh(1).
func parseTarget(t string) (userName, addr string, err error) {
	userName, host, ok := strings.Cut(t, "@")
	if !ok {
		host = userName
		userName = ""
	}
	if host == "" {
		return "", "", fmt.Errorf("target %q has no host", t)
	}
	if userName == "" {
		if u := os.Getenv("USER"); u != "" {
			userName = u
		} else if u, err := user.Current(); err == nil {
			userName = u.Username
		} else {
			return "", "", errors.New("cannot tell the local user name; use user@host")
		}
	}
	if _, _, err := net.SplitHostPort(host); err != nil {
		host = net.JoinHostPort(strings.Trim(host, "[]"), "22")
	}
	return userName, host, nil
}

// remoteCommand joins the remaining args the way ssh(1) does: with spaces, and
// no quoting of its own. hc passes a ready-made shell line as one argument.
func remoteCommand(args []string) string { return strings.Join(args, " ") }

// isCheckBanner spots Tailscale SSH "check" mode, which holds the session
// until someone approves it in a browser. Same test as nixcfg-mcp's hostprobe.
func isCheckBanner(msg string) bool {
	return strings.Contains(msg, "login.tailscale.com") || strings.Contains(msg, "additional check")
}

// handshakeTimeout bounds the SSH handshake; a check-mode server would
// otherwise sit in auth forever, and --via must never wait on a person.
const handshakeTimeout = 30 * time.Second

func runSSH(slot, target string, args []string) error {
	if len(args) == 0 {
		return &exitError{2, "usage: hc-tailscale [--slot S] ssh [user@]host[:port] <command...> (an interactive shell is not supported)"}
	}
	userName, addr, err := parseTarget(target)
	if err != nil {
		return &exitError{2, err.Error()}
	}

	c, err := connectDaemon()
	if err != nil {
		return &exitError{exitTransport, err.Error()}
	}
	r, err := roundTrip(c, request{Op: "dial", Slot: slot, Addr: addr})
	if err != nil {
		_ = c.Close()
		return &exitError{exitTransport, err.Error()}
	}
	if !r.OK {
		_ = c.Close()
		return &exitError{exitTransport, replyErr(slot, r).Error()}
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	var checkMsg string
	cfg := &ssh.ClientConfig{
		User: userName,
		// nil Auth: x/crypto/ssh always tries "none" first, which is what
		// Tailscale SSH accepts -- the peer is authenticated at the WireGuard
		// layer, as this slot's node. For the same reason the host key is left
		// to Tailscale: the stream is already authenticated and encrypted end
		// to end, and there is no known_hosts for tsnet peers to check against.
		Auth:            nil,
		HostKeyCallback: ssh.InsecureIgnoreHostKey(),
		Timeout:         handshakeTimeout,
		BannerCallback: func(msg string) error {
			if isCheckBanner(msg) {
				checkMsg = strings.TrimSpace(msg)
				cancel()
				return errors.New("tailscale ssh check required")
			}
			return nil
		},
	}

	stop := context.AfterFunc(ctx, func() { _ = c.Close() })
	defer stop()
	_ = c.SetDeadline(time.Now().Add(handshakeTimeout))
	sc, chans, reqs, err := ssh.NewClientConn(c, addr, cfg)
	if err != nil {
		_ = c.Close()
		if checkMsg != "" {
			return &exitError{exitTransport, fmt.Sprintf("%s: Tailscale SSH wants a browser check first:\n%s\nApprove it, then retry.", target, checkMsg)}
		}
		return &exitError{exitTransport, fmt.Sprintf("ssh %s: %v", target, err)}
	}
	_ = c.SetDeadline(time.Time{})
	client := ssh.NewClient(sc, chans, reqs)
	defer client.Close()

	sess, err := client.NewSession()
	if err != nil {
		return &exitError{exitTransport, fmt.Sprintf("ssh %s: %v", target, err)}
	}
	defer sess.Close()

	// Bytes in both directions untouched: no PTY, no escape character, and
	// stderr kept apart from stdout, as hc's --via contract asks.
	sess.Stdin = os.Stdin
	sess.Stdout = os.Stdout
	sess.Stderr = os.Stderr

	err = sess.Run(remoteCommand(args))
	if err == nil {
		return nil
	}
	var ee *ssh.ExitError
	if errors.As(err, &ee) {
		if ee.Signal() != "" {
			return &exitError{exitTransport, ""}
		}
		return &exitError{ee.ExitStatus(), ""}
	}
	return &exitError{exitTransport, fmt.Sprintf("ssh %s: %v", target, err)}
}

func runLogin(slot, hostname string, out io.Writer) error {
	c, err := connectDaemon()
	if err != nil {
		return err
	}
	defer c.Close()
	if err := writeJSONLine(c, request{Op: "login", Slot: slot, Hostname: hostname}); err != nil {
		return err
	}
	opened := false
	for {
		var r reply
		if err := readJSONLine(c, &r); err != nil {
			return fmt.Errorf("daemon: %w", err)
		}
		if !r.OK {
			return replyErr(slot, r)
		}
		if r.AuthURL != "" {
			fmt.Fprintf(out, "To sign slot %q into Tailscale, open:\n\n  %s\n\n", slot, r.AuthURL)
			if !opened {
				opened = true
				if p, err := exec.LookPath("xdg-open"); err == nil && os.Getenv("DISPLAY")+os.Getenv("WAYLAND_DISPLAY") != "" {
					_ = exec.Command(p, r.AuthURL).Start()
				}
			}
			fmt.Fprintln(out, "Waiting for the login to finish (ctrl+c to give up)...")
		}
		if r.Done {
			for _, s := range r.Slots {
				printStatus(out, s)
			}
			return nil
		}
	}
}

func runSimple(op, slot string, out io.Writer) error {
	c, err := connectDaemon()
	if err != nil {
		return err
	}
	defer c.Close()
	r, err := roundTrip(c, request{Op: op, Slot: slot})
	if err != nil {
		return err
	}
	if !r.OK {
		return replyErr(slot, r)
	}
	if op == "status" && len(r.Slots) == 0 {
		fmt.Fprintln(out, "no slots yet; run: hc-tailscale login")
	}
	for _, s := range r.Slots {
		printStatus(out, s)
	}
	if op == "logout" {
		fmt.Fprintf(out, "slot %q logged out\n", slot)
	}
	return nil
}

func printStatus(out io.Writer, s slotStatus) {
	fmt.Fprintf(out, "%s\t%s", s.Slot, s.State)
	if s.Tailnet != "" {
		fmt.Fprintf(out, "\ttailnet=%s", s.Tailnet)
	}
	if s.DNSName != "" {
		fmt.Fprintf(out, "\tname=%s", strings.TrimSuffix(s.DNSName, "."))
	} else if s.Hostname != "" {
		fmt.Fprintf(out, "\thostname=%s", s.Hostname)
	}
	if len(s.IPs) > 0 {
		fmt.Fprintf(out, "\tips=%s", strings.Join(s.IPs, ","))
	}
	if s.AuthURL != "" && s.State != "Running" {
		fmt.Fprintf(out, "\tlogin=%s", s.AuthURL)
	}
	fmt.Fprintln(out)
}
