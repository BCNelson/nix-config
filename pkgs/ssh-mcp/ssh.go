package main

import (
	"context"
	"net"
	"sync"
	"time"

	"golang.org/x/crypto/ssh"
)

// maxOutput caps how much command output we retain in memory per command. When
// exceeded we keep the most recent bytes (the tail) and flag truncation, since
// the end of a command's output is usually the most relevant part.
const maxOutput = 256 * 1024

// target is a configured, allow-listed SSH destination.
type target struct {
	name string // friendly name used by tools (e.g. "romeo")
	user string // SSH user
	addr string // host:port
}

// outputBuffer is a concurrency-safe, size-bounded sink for command output.
// stdout and stderr are merged into a single stream so ordering reads like a
// terminal. Once the cap is hit it retains only the trailing maxOutput bytes.
type outputBuffer struct {
	mu        sync.Mutex
	buf       []byte
	cap       int
	total     int64
	truncated bool
}

func newOutputBuffer(cap int) *outputBuffer {
	return &outputBuffer{cap: cap}
}

func (b *outputBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.total += int64(len(p))
	b.buf = append(b.buf, p...)
	if len(b.buf) > b.cap {
		b.truncated = true
		b.buf = b.buf[len(b.buf)-b.cap:]
	}
	return len(p), nil
}

// snapshot returns the retained output, whether it was truncated, and the total
// number of bytes seen (including any dropped by truncation).
func (b *outputBuffer) snapshot() (string, bool, int64) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return string(b.buf), b.truncated, b.total
}

// dial opens an SSH connection to tgt.
//
// Auth is intentionally nil: golang.org/x/crypto/ssh always attempts the "none"
// auth method first, which is exactly what Tailscale SSH accepts — the peer is
// already authenticated at the WireGuard layer, so no key or password is sent.
// For the same reason host-key verification is delegated to Tailscale (the
// tunnel is already authenticated and encrypted), so InsecureIgnoreHostKey is
// acceptable here and avoids needing a known_hosts file.
func dial(ctx context.Context, tgt target) (*ssh.Client, error) {
	cfg := &ssh.ClientConfig{
		User:            tgt.user,
		Auth:            nil,
		HostKeyCallback: ssh.InsecureIgnoreHostKey(),
		Timeout:         15 * time.Second,
	}

	d := net.Dialer{Timeout: cfg.Timeout}
	conn, err := d.DialContext(ctx, "tcp", tgt.addr)
	if err != nil {
		return nil, err
	}

	c, chans, reqs, err := ssh.NewClientConn(conn, tgt.addr, cfg)
	if err != nil {
		_ = conn.Close()
		return nil, err
	}
	return ssh.NewClient(c, chans, reqs), nil
}

// runCommand connects to tgt, runs command, and streams combined stdout/stderr
// into out. It blocks until the command finishes or ctx is canceled. The return
// value is the remote exit code; a non-nil error indicates a transport/connection
// problem or cancellation rather than a non-zero exit (a command that exits
// non-zero returns (code, nil)).
func runCommand(ctx context.Context, tgt target, command string, out *outputBuffer) (int, error) {
	client, err := dial(ctx, tgt)
	if err != nil {
		return -1, err
	}
	defer client.Close()

	session, err := client.NewSession()
	if err != nil {
		return -1, err
	}
	defer session.Close()

	session.Stdout = out
	session.Stderr = out

	if err := session.Start(command); err != nil {
		return -1, err
	}

	done := make(chan error, 1)
	go func() { done <- session.Wait() }()

	select {
	case <-ctx.Done():
		// Best effort: ask the remote to die, then tear down the session so
		// session.Wait() unblocks.
		_ = session.Signal(ssh.SIGKILL)
		_ = session.Close()
		<-done
		return -1, ctx.Err()
	case werr := <-done:
		if werr == nil {
			return 0, nil
		}
		if ee, ok := werr.(*ssh.ExitError); ok {
			return ee.ExitStatus(), nil
		}
		// e.g. *ssh.ExitMissingError when the server closed without a status.
		return -1, werr
	}
}
