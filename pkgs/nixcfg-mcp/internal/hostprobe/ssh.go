// Package hostprobe reads a host's auto-update and boot state in one ssh round
// trip, with hard deadlines. Adapted from pkgs/ssh-mcp: keyless Tailscale SSH,
// plus detection of Tailscale's "check" mode, which otherwise hangs a
// non-interactive connection until someone completes a browser login.
package hostprobe

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"net"
	"strings"
	"sync"
	"time"

	"golang.org/x/crypto/ssh"
)

// ErrNeedsLogin means Tailscale SSH wants an interactive browser check.
var ErrNeedsLogin = errors.New("tailscale ssh wants a browser login (check mode)")

const (
	dialTimeout = 15 * time.Second
	maxOutput   = 1 << 20
)

// Exec runs command on user@host and returns combined output and exit code.
//
// Auth is nil: x/crypto/ssh always tries "none" first, which is what Tailscale
// SSH accepts (the peer is authenticated at the WireGuard layer). Host keys are
// likewise left to Tailscale.
func Exec(ctx context.Context, dest, command string) (string, int, error) {
	user, addr, ok := strings.Cut(dest, "@")
	if !ok {
		return "", -1, fmt.Errorf("ssh destination %q is not user@host", dest)
	}
	if !strings.Contains(addr, ":") {
		addr += ":22"
	}

	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	needsLogin := make(chan struct{}, 1)
	cfg := &ssh.ClientConfig{
		User:            user,
		HostKeyCallback: ssh.InsecureIgnoreHostKey(),
		Timeout:         dialTimeout,
		BannerCallback: func(msg string) error {
			if strings.Contains(msg, "login.tailscale.com") || strings.Contains(msg, "additional check") {
				select {
				case needsLogin <- struct{}{}:
				default:
				}
				cancel()
				return ErrNeedsLogin
			}
			return nil
		},
	}

	d := net.Dialer{Timeout: dialTimeout}
	conn, err := d.DialContext(ctx, "tcp", addr)
	if err != nil {
		return "", -1, err
	}
	// Bound the handshake too: a check-mode server can sit in auth forever.
	stopWatch := context.AfterFunc(ctx, func() { _ = conn.Close() })
	defer stopWatch()
	_ = conn.SetDeadline(time.Now().Add(dialTimeout + 15*time.Second))

	c, chans, reqs, err := ssh.NewClientConn(conn, addr, cfg)
	if err != nil {
		_ = conn.Close()
		select {
		case <-needsLogin:
			return "", -1, ErrNeedsLogin
		default:
		}
		if errors.Is(err, ErrNeedsLogin) {
			return "", -1, ErrNeedsLogin
		}
		return "", -1, err
	}
	_ = conn.SetDeadline(time.Time{})
	client := ssh.NewClient(c, chans, reqs)
	defer client.Close()

	session, err := client.NewSession()
	if err != nil {
		return "", -1, err
	}
	defer session.Close()

	var out limitedBuffer
	session.Stdout = &out
	session.Stderr = &out
	if err := session.Start(command); err != nil {
		return "", -1, err
	}
	done := make(chan error, 1)
	go func() { done <- session.Wait() }()
	select {
	case <-ctx.Done():
		_ = session.Signal(ssh.SIGKILL)
		_ = session.Close()
		<-done
		return out.String(), -1, ctx.Err()
	case werr := <-done:
		if werr == nil {
			return out.String(), 0, nil
		}
		var ee *ssh.ExitError
		if errors.As(werr, &ee) {
			return out.String(), ee.ExitStatus(), nil
		}
		return out.String(), -1, werr
	}
}

// limitedBuffer is written by the stdout and stderr copiers concurrently.
type limitedBuffer struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (b *limitedBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	n := len(p)
	if b.buf.Len()+len(p) > maxOutput {
		p = p[:max(0, maxOutput-b.buf.Len())]
	}
	b.buf.Write(p)
	return n, nil
}

func (b *limitedBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.String()
}
