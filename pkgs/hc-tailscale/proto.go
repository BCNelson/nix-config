package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
)

// The daemon and its clients talk over a unix socket. Every exchange starts
// with one JSON request line from the client and at least one JSON reply line
// from the daemon. For "dial", a successful reply is followed by the raw TCP
// stream in both directions; for "login", the daemon may send several replies
// (each new auth URL) before the final one with Done set.

type request struct {
	Op       string `json:"op"`
	Slot     string `json:"slot,omitempty"`
	Addr     string `json:"addr,omitempty"`
	Hostname string `json:"hostname,omitempty"`
}

type slotStatus struct {
	Slot     string   `json:"slot"`
	Hostname string   `json:"hostname,omitempty"`
	State    string   `json:"state"`
	Tailnet  string   `json:"tailnet,omitempty"`
	DNSName  string   `json:"dns_name,omitempty"`
	IPs      []string `json:"ips,omitempty"`
	AuthURL  string   `json:"auth_url,omitempty"`
}

type reply struct {
	OK      bool         `json:"ok"`
	Done    bool         `json:"done,omitempty"`
	Code    string       `json:"code,omitempty"`
	Error   string       `json:"error,omitempty"`
	AuthURL string       `json:"auth_url,omitempty"`
	Slots   []slotStatus `json:"slots,omitempty"`
}

// Error codes a client can act on.
const (
	codeNeedsLogin  = "needs_login"
	codeMachineAuth = "needs_machine_auth"
	codeBadRequest  = "bad_request"
	codeDial        = "dial_failed"
	codeInternal    = "internal"
)

const defaultSlot = "default"

var slotRE = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{0,62}$`)

func validSlot(s string) error {
	if !slotRE.MatchString(s) {
		return fmt.Errorf("slot %q: use lowercase letters, digits and '-' (max 63)", s)
	}
	return nil
}

// readLine reads one '\n'-terminated line a byte at a time. It must not read
// ahead: on a dial, whatever follows the line belongs to the spliced stream,
// and a buffered reader would swallow the start of it.
func readLine(r io.Reader) ([]byte, error) {
	var line []byte
	var b [1]byte
	for {
		n, err := r.Read(b[:])
		if n == 1 {
			if b[0] == '\n' {
				return line, nil
			}
			line = append(line, b[0])
			if len(line) > 64*1024 {
				return nil, errors.New("line too long")
			}
		}
		if err != nil {
			if err == io.EOF && len(line) > 0 {
				return nil, io.ErrUnexpectedEOF
			}
			return nil, err
		}
	}
}

func writeJSONLine(w io.Writer, v any) error {
	b, err := json.Marshal(v)
	if err != nil {
		return err
	}
	_, err = w.Write(append(b, '\n'))
	return err
}

func readJSONLine(r io.Reader, v any) error {
	line, err := readLine(r)
	if err != nil {
		return err
	}
	return json.Unmarshal(line, v)
}

// socketPath is where the daemon listens. HC_TAILSCALE_SOCKET overrides it
// (tests, or a second daemon by hand).
func socketPath() (string, error) {
	if p := os.Getenv("HC_TAILSCALE_SOCKET"); p != "" {
		return p, nil
	}
	if d := os.Getenv("XDG_RUNTIME_DIR"); d != "" {
		return filepath.Join(d, "hc-tailscale.sock"), nil
	}
	return "", errors.New("XDG_RUNTIME_DIR is not set; set HC_TAILSCALE_SOCKET")
}

// stateBase holds one subdirectory per slot. systemd's StateDirectory= sets
// STATE_DIRECTORY, which wins; otherwise the XDG location it would have used.
func stateBase() (string, error) {
	if d := os.Getenv("STATE_DIRECTORY"); d != "" {
		return d, nil
	}
	if d := os.Getenv("XDG_STATE_HOME"); d != "" {
		return filepath.Join(d, "hc-tailscale"), nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".local", "state", "hc-tailscale"), nil
}
