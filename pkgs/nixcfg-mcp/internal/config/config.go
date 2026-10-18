// Package config locates the nix-config checkout and loads .agents/hosts.json.
package config

import (
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

// Host is one entry of .agents/hosts.json.
type Host struct {
	Name     string `json:"-"`
	SSH      string `json:"ssh,omitempty"`
	Checkout string `json:"checkout,omitempty"`
	Interval string `json:"interval,omitempty"`
	Ntfy     bool   `json:"ntfy,omitempty"`
	Reboot   bool   `json:"reboot,omitempty"`
	Preseed  bool   `json:"preseed,omitempty"`
	// AutoUpdate is nil (true) unless the host is explicitly not auto-updating.
	AutoUpdate *bool  `json:"auto_update,omitempty"`
	Note       string `json:"note,omitempty"`
}

// Observable reports whether nixcfg can ssh in to check the host.
func (h Host) Observable() bool { return h.SSH != "" }

// AutoUpdating reports whether the host runs auto-update at all.
func (h Host) AutoUpdating() bool { return h.AutoUpdate == nil || *h.AutoUpdate }

// IntervalDuration parses Interval ("1h", "5m"); zero if unset or invalid.
func (h Host) IntervalDuration() time.Duration {
	d, _ := time.ParseDuration(h.Interval)
	return d
}

// SSHHost is the address part of SSH ("romeo.b.nel.family").
func (h Host) SSHHost() string {
	_, host, ok := strings.Cut(h.SSH, "@")
	if !ok {
		return h.SSH
	}
	return host
}

// SSHUser is the user part of SSH.
func (h Host) SSHUser() string {
	user, _, ok := strings.Cut(h.SSH, "@")
	if !ok {
		return ""
	}
	return user
}

// Forgejo identifies the CI server.
type Forgejo struct {
	URL      string `json:"url"`
	Repo     string `json:"repo"`
	Workflow string `json:"workflow"`
}

// Config is the loaded repository context.
type Config struct {
	Root    string
	Forgejo Forgejo
	Builder string
	Hosts   map[string]Host
}

// FindRoot resolves the repository: explicit, $NIXCFG_REPO, or the git
// top-level of the working directory.
func FindRoot(explicit string) (string, error) {
	if explicit != "" {
		return filepath.Abs(explicit)
	}
	if env := os.Getenv("NIXCFG_REPO"); env != "" {
		return filepath.Abs(env)
	}
	out, err := exec.Command("git", "rev-parse", "--show-toplevel").Output()
	if err != nil {
		return "", fmt.Errorf("not inside the nix-config checkout (set NIXCFG_REPO or pass --repo)")
	}
	return strings.TrimSpace(string(out)), nil
}

// Load reads .agents/hosts.json under root.
func Load(root string) (*Config, error) {
	raw, err := os.ReadFile(filepath.Join(root, ".agents", "hosts.json"))
	if err != nil {
		return nil, fmt.Errorf("reading .agents/hosts.json: %w (is %s the nix-config checkout?)", err, root)
	}
	var f struct {
		Forgejo Forgejo         `json:"forgejo"`
		Builder string          `json:"builder"`
		Hosts   map[string]Host `json:"hosts"`
	}
	if err := json.Unmarshal(raw, &f); err != nil {
		return nil, fmt.Errorf("parsing .agents/hosts.json: %w", err)
	}
	for name, h := range f.Hosts {
		h.Name = name
		f.Hosts[name] = h
	}
	if f.Forgejo.URL == "" {
		f.Forgejo = Forgejo{URL: "https://git.bcnelson.dev", Repo: "bcnelson/nix-config", Workflow: "check.yml"}
	}
	return &Config{Root: root, Forgejo: f.Forgejo, Builder: f.Builder, Hosts: f.Hosts}, nil
}

// HostNames returns the configured host names, sorted.
func (c *Config) HostNames() []string {
	names := make([]string, 0, len(c.Hosts))
	for n := range c.Hosts {
		names = append(names, n)
	}
	sort.Strings(names)
	return names
}

// Host looks up a host by flake name, also accepting the ssh short name
// ("romeo" for romeo-2) when that is unambiguous.
func (c *Config) Host(name string) (Host, error) {
	if h, ok := c.Hosts[name]; ok {
		return h, nil
	}
	var found []Host
	for _, h := range c.Hosts {
		if strings.HasPrefix(h.Name, name+"-") || (h.SSH != "" && strings.HasPrefix(h.SSHHost(), name+".")) {
			found = append(found, h)
		}
	}
	if len(found) == 1 {
		return found[0], nil
	}
	return Host{}, fmt.Errorf("unknown host %q; known: %s", name, strings.Join(c.HostNames(), ", "))
}

// StateDir is where waits and jobs persist ($XDG_STATE_HOME/nixcfg).
func StateDir(kind string) string {
	base := os.Getenv("XDG_STATE_HOME")
	if base == "" {
		home, _ := os.UserHomeDir()
		base = filepath.Join(home, ".local", "state")
	}
	return filepath.Join(base, "nixcfg", kind)
}
