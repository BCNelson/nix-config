package hostprobe

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"sync"
	"time"
)

// Peer is what the local tailscaled knows about a host.
type Peer struct {
	HostName string    `json:"host_name"`
	Online   bool      `json:"online"`
	LastSeen time.Time `json:"last_seen,omitempty"`
}

// OfflineError is returned instead of dialing a host the tailnet says is
// offline: dialing it only ever times out.
type OfflineError struct {
	Host     string
	LastSeen time.Time
}

func (e *OfflineError) Error() string {
	if e.LastSeen.IsZero() || e.LastSeen.Year() < 2000 {
		return fmt.Sprintf("%s is offline on the tailnet", e.Host)
	}
	return fmt.Sprintf("%s is offline on the tailnet (last seen %s ago)", e.Host, Ago(time.Since(e.LastSeen)))
}

// Ago renders a duration the way people say it: 184d, 5h, 12m.
func Ago(d time.Duration) string {
	switch {
	case d >= 48*time.Hour:
		return fmt.Sprintf("%dd", int(d.Hours()/24))
	case d >= 2*time.Hour:
		return fmt.Sprintf("%dh", int(d.Hours()))
	default:
		return d.Round(time.Minute).String()
	}
}

var (
	tailnetMu    sync.Mutex
	tailnetAt    time.Time
	tailnetPeers map[string]Peer
)

// Tailnet returns the tailnet's peers keyed by host name (which is the flake
// host name here: romeo-2, charlie-1). It is cached for 30s; nil when
// tailscale isn't available, in which case callers just dial.
func Tailnet(ctx context.Context) map[string]Peer {
	tailnetMu.Lock()
	defer tailnetMu.Unlock()
	if time.Since(tailnetAt) < 30*time.Second {
		return tailnetPeers
	}
	tailnetAt = time.Now()
	tailnetPeers = nil

	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, "tailscale", "status", "--json").Output()
	if err != nil {
		return nil
	}
	var st struct {
		Peer map[string]struct {
			HostName string    `json:"HostName"`
			Online   bool      `json:"Online"`
			LastSeen time.Time `json:"LastSeen"`
		} `json:"Peer"`
	}
	if json.Unmarshal(out, &st) != nil {
		return nil
	}
	peers := map[string]Peer{}
	for _, p := range st.Peer {
		peers[p.HostName] = Peer{HostName: p.HostName, Online: p.Online, LastSeen: p.LastSeen}
	}
	tailnetPeers = peers
	return peers
}

// Offline reports whether the tailnet knows host and says it is offline.
func Offline(ctx context.Context, host string) *OfflineError {
	p, ok := Tailnet(ctx)[host]
	if !ok || p.Online {
		return nil
	}
	return &OfflineError{Host: host, LastSeen: p.LastSeen}
}
