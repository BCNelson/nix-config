// Package humancmd builds the one-line commands agents hand to the human. They
// are always a single line: no backslash continuations.
package humancmd

import (
	"fmt"
	"sort"
	"strings"

	"nixcfg-mcp/internal/config"
)

func dest(h config.Host) string {
	if h.SSH != "" {
		return h.SSH
	}
	return "bcnelson@" + h.Name
}

func checkout(h config.Host) string {
	if h.Checkout != "" {
		return h.Checkout
	}
	return "/config"
}

// Push hands over a push to main.
func Push() string { return "git push origin main" }

// Kick starts auto-update on a host now.
func Kick(h config.Host) string {
	return fmt.Sprintf("ssh -t %s sudo systemctl start --no-block auto-update.service", dest(h))
}

// ResetRetries zeroes the attempt counter (keeping BUILT_COMMIT) and starts a run.
func ResetRetries(h config.Host) string {
	return fmt.Sprintf("ssh -t %s \"sudo sed -i 's/^ATTEMPT_COUNT=.*/ATTEMPT_COUNT=0/' /var/lib/auto-update/state && sudo systemctl start --no-block auto-update.service\"", dest(h))
}

// Reboot reboots a host.
func Reboot(h config.Host) string {
	return fmt.Sprintf("ssh -t %s sudo systemctl reboot", dest(h))
}

// SwitchBranch puts the host checkout back on auto-update.
func SwitchBranch(h config.Host, path string) string {
	if path == "" {
		path = checkout(h)
	}
	return fmt.Sprintf("ssh -t %s sudo git -C %s switch auto-update", dest(h), path)
}

// SSHLogin completes a Tailscale SSH check-mode login.
func SSHLogin(h config.Host) string {
	return fmt.Sprintf("ssh %s true", dest(h))
}

// Journal shows the last auto-update run.
func Journal(h config.Host) string {
	return fmt.Sprintf("ssh %s journalctl -u auto-update.service -n 200 --no-pager", dest(h))
}

// RerunCI is the Forgejo page to dispatch check.yml.
func RerunCI(cfg *config.Config) string {
	return fmt.Sprintf("open %s/%s/actions?workflow=%s and press \"Run workflow\" on main", cfg.Forgejo.URL, cfg.Forgejo.Repo, cfg.Forgejo.Workflow)
}

// Action is one named hand-off.
type Action struct {
	Command       string `json:"command"`
	Why           string `json:"why"`
	NeedsKeyTouch bool   `json:"needs_key_touch,omitempty"`
}

// Actions lists the named hand-offs (`nixcfg human <action> [host]`).
var Actions = map[string]string{
	"push":          "push main (deploys once CI passes)",
	"kick":          "start auto-update on a host now",
	"reset-retries": "let a host retry a commit whose retries are exhausted",
	"reboot":        "reboot a host",
	"switch-branch": "put a host checkout back on auto-update",
	"ssh-login":     "complete a Tailscale SSH check-mode login",
	"journal":       "show the last auto-update run on a host",
	"rerun-ci":      "re-run check.yml on main",
	"rekey":         "re-encrypt secrets for every host (hardware key touch)",
	"new-secrets":   "generate and rekey newly declared secrets (hardware key touch)",
	"edit-secret":   "create or edit a hand-entered secret (hardware key touch)",
	"unlock":        "unlock git-crypt (GPG key)",
	"switch":        "activate this machine's configuration",
}

// ActionNames is Actions' keys, sorted.
func ActionNames() []string {
	names := make([]string, 0, len(Actions))
	for k := range Actions {
		names = append(names, k)
	}
	sort.Strings(names)
	return names
}

// Build resolves a named hand-off.
func Build(cfg *config.Config, action, host, arg string) (Action, error) {
	needHost := func() (config.Host, error) {
		if host == "" {
			return config.Host{}, fmt.Errorf("%s needs a host", action)
		}
		return cfg.Host(host)
	}
	why := Actions[action]
	switch action {
	case "push":
		return Action{Command: Push(), Why: why}, nil
	case "kick", "reset-retries", "reboot", "switch-branch", "ssh-login", "journal":
		h, err := needHost()
		if err != nil {
			return Action{}, err
		}
		cmd := map[string]func(config.Host) string{
			"kick": Kick, "reset-retries": ResetRetries, "reboot": Reboot, "ssh-login": SSHLogin, "journal": Journal,
			"switch-branch": func(h config.Host) string { return SwitchBranch(h, "") },
		}[action](h)
		return Action{Command: cmd, Why: why}, nil
	case "rerun-ci":
		return Action{Command: RerunCI(cfg), Why: why}, nil
	case "rekey":
		return Action{Command: "just rekey", Why: why, NeedsKeyTouch: true}, nil
	case "new-secrets":
		return Action{Command: "just new-secrets", Why: why, NeedsKeyTouch: true}, nil
	case "edit-secret":
		if arg == "" {
			arg = "<path>.age"
		}
		return Action{Command: "agenix edit " + arg + " && just rekey", Why: why, NeedsKeyTouch: true}, nil
	case "unlock":
		return Action{Command: "just unlock", Why: why, NeedsKeyTouch: true}, nil
	case "switch":
		return Action{Command: "just update-os", Why: why}, nil
	}
	return Action{}, fmt.Errorf("unknown action %q; one of: %s", action, strings.Join(ActionNames(), ", "))
}
