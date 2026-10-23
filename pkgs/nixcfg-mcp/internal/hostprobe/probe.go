package hostprobe

import (
	"context"
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
	"time"
)

// probeScript prints key=value lines. It only reads.
const probeScript = `set +e
S=/var/lib/auto-update/state
if [ -r "$S" ]; then sed -n 's/^\([A-Z_]*\)=\(.*\)$/state.\1=\2/p' "$S"; else echo state.missing=1; fi
systemctl show auto-update.service --timestamp=unix -p LoadState,ActiveState,SubState,Result,ExecMainStatus,ExecMainStartTimestamp,ExecMainExitTimestamp,InvocationID | sed 's/^/svc./'
env=$(systemctl show auto-update.service -p Environment --value)
for kv in $env; do case $kv in CONFIG_PATH=*|MAX_RETRIES=*|REBOOT=*) echo "env.$kv";; esac; done
P=$(for kv in $env; do case $kv in CONFIG_PATH=*) echo "${kv#CONFIG_PATH=}";; esac; done)
echo "ntfy=$(systemctl is-active autoupdate-ntfy-client.service 2>/dev/null)"
echo "timer.json=$(systemctl list-timers auto-update.timer --all --output=json 2>/dev/null | tr -d '\n')"
if [ -n "$P" ] && [ -d "$P" ]; then
  # The checkout is often root's; reading it as another user trips git's
  # dubious-ownership check unless it is waived for this one command.
  g() { git -c safe.directory="$P" -C "$P" "$@" 2>/dev/null; }
  echo "git.branch=$(g symbolic-ref -q --short HEAD)"
  echo "git.head=$(g rev-parse -q --verify HEAD)"
  g config --get filter.git-crypt.smudge >/dev/null && echo git.crypt_unlocked=1
  echo git.readable=$(g rev-parse --git-dir >/dev/null && echo 1 || echo 0)
fi
echo "sys.current=$(readlink /run/current-system)"
echo "sys.profile=$(readlink -f /nix/var/nix/profiles/system)"
echo "sys.booted=$(readlink /run/booted-system)"
for f in initrd kernel kernel-modules; do
  echo "sys.cur.$f=$(readlink /run/current-system/$f)"
  echo "sys.boot.$f=$(readlink /run/booted-system/$f)"
done
echo "sys.bootid=$(cat /proc/sys/kernel/random/boot_id)"
echo "sys.state=$(systemctl is-system-running 2>/dev/null)"
[ -e /run/systemd/shutdown/scheduled ] && echo sys.reboot_scheduled=1
echo "sys.now=$(date +%s)"
echo probe.done=1
`

// State is one observation of a host.
type State struct {
	Host     string    `json:"host"`
	At       time.Time `json:"at"`
	Enabled  bool      `json:"auto_update_enabled"`
	Checkout string    `json:"checkout,omitempty"`
	Branch   string    `json:"branch,omitempty"`
	Head     string    `json:"head,omitempty"`
	Unlocked bool      `json:"git_crypt_unlocked"`

	BuiltCommit   string `json:"built_commit,omitempty"`
	AttemptCommit string `json:"attempt_commit,omitempty"`
	AttemptCount  int    `json:"attempt_count"`
	MaxRetries    int    `json:"max_retries"`
	Reboot        bool   `json:"reboot"`

	Active      string    `json:"service_active"` // active | activating | inactive | failed
	Result      string    `json:"service_result"` // success | exit-code | timeout | ...
	ExitStatus  int       `json:"service_exit_status"`
	StartedAt   time.Time `json:"service_started_at,omitempty"`
	ExitedAt    time.Time `json:"service_exited_at,omitempty"`
	Invocation  string    `json:"invocation_id,omitempty"`
	Ntfy        bool      `json:"ntfy_subscriber"`
	NextTimer   time.Time `json:"next_timer,omitempty"`
	LastTrigger time.Time `json:"last_trigger,omitempty"`

	Current         string `json:"current_system"`
	Profile         string `json:"system_profile"`
	Booted          string `json:"booted_system"`
	KernelChanged   bool   `json:"kernel_changed"` // current vs booted kernel/initrd/modules
	BootID          string `json:"boot_id"`
	SystemState     string `json:"system_state"`
	RebootScheduled bool   `json:"reboot_scheduled"`
}

// Running reports whether auto-update is mid-run.
func (s State) Running() bool {
	return s.Active == "activating" || s.Active == "active" || s.Active == "reloading"
}

// SwitchPending: the profile moved but the running system didn't (the
// switchInhibitors "boot" fallback). A reboot will activate it.
func (s State) SwitchPending() bool {
	return s.Profile != "" && s.Current != "" && s.Profile != s.Current
}

// NeedsReboot: something only a reboot applies is waiting.
func (s State) NeedsReboot() bool { return s.SwitchPending() || s.KernelChanged }

// Probe observes a host.
func Probe(ctx context.Context, host, dest string) (State, error) {
	if off := Offline(ctx, host); off != nil {
		return State{}, off
	}
	ctx, cancel := context.WithTimeout(ctx, 45*time.Second)
	defer cancel()
	out, code, err := Exec(ctx, dest, probeScript)
	if err != nil {
		return State{}, err
	}
	kv := parseKV(out)
	if kv["probe.done"] != "1" {
		return State{}, fmt.Errorf("probe did not finish (exit %d): %s", code, lastLines(out, 5))
	}
	return toState(host, kv), nil
}

func parseKV(out string) map[string]string {
	kv := map[string]string{}
	for _, l := range strings.Split(out, "\n") {
		k, v, ok := strings.Cut(l, "=")
		if ok {
			kv[strings.TrimSpace(k)] = strings.TrimSpace(v)
		}
	}
	return kv
}

func toState(host string, kv map[string]string) State {
	s := State{
		Host:          host,
		At:            unix(kv["sys.now"]),
		Enabled:       kv["svc.LoadState"] == "loaded",
		Checkout:      kv["env.CONFIG_PATH"],
		Branch:        kv["git.branch"],
		Head:          kv["git.head"],
		Unlocked:      kv["git.crypt_unlocked"] == "1" || kv["git.readable"] != "1",
		BuiltCommit:   kv["state.BUILT_COMMIT"],
		AttemptCommit: kv["state.ATTEMPT_COMMIT"],
		Active:        kv["svc.ActiveState"],
		Result:        kv["svc.Result"],
		Invocation:    kv["svc.InvocationID"],
		Ntfy:          kv["ntfy"] == "active",
		Current:       kv["sys.current"],
		Profile:       kv["sys.profile"],
		Booted:        kv["sys.booted"],
		BootID:        kv["sys.bootid"],
		SystemState:   kv["sys.state"],
		Reboot:        truthy(kv["env.REBOOT"]),
	}
	if s.At.IsZero() {
		s.At = time.Now()
	}
	s.AttemptCount, _ = strconv.Atoi(kv["state.ATTEMPT_COUNT"])
	s.MaxRetries, _ = strconv.Atoi(kv["env.MAX_RETRIES"])
	if s.MaxRetries == 0 {
		s.MaxRetries = 3
	}
	s.ExitStatus, _ = strconv.Atoi(kv["svc.ExecMainStatus"])
	s.StartedAt = unix(strings.TrimPrefix(kv["svc.ExecMainStartTimestamp"], "@"))
	s.ExitedAt = unix(strings.TrimPrefix(kv["svc.ExecMainExitTimestamp"], "@"))
	for _, f := range []string{"initrd", "kernel", "kernel-modules"} {
		if kv["sys.cur."+f] != kv["sys.boot."+f] {
			s.KernelChanged = true
		}
	}
	s.RebootScheduled = kv["sys.reboot_scheduled"] == "1"

	var timers []struct {
		Next int64 `json:"next"`
		Last int64 `json:"last"`
	}
	if err := json.Unmarshal([]byte(kv["timer.json"]), &timers); err == nil && len(timers) > 0 {
		if timers[0].Next > 0 {
			s.NextTimer = time.UnixMicro(timers[0].Next)
		}
		if timers[0].Last > 0 {
			s.LastTrigger = time.UnixMicro(timers[0].Last)
		}
	}
	return s
}

func truthy(s string) bool {
	switch strings.ToLower(s) {
	case "1", "true", "yes", "y":
		return true
	}
	return false
}

func unix(s string) time.Time {
	s = strings.TrimPrefix(s, "@")
	if n, err := strconv.ParseInt(s, 10, 64); err == nil && n > 0 {
		return time.Unix(n, 0)
	}
	return time.Time{}
}

func lastLines(s string, n int) string {
	l := strings.Split(strings.TrimRight(s, "\n"), "\n")
	if len(l) > n {
		l = l[len(l)-n:]
	}
	return strings.Join(l, "\n")
}

// Journal returns the log of one auto-update invocation (or the last few runs).
func Journal(ctx context.Context, dest, invocation string, lines int) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, 45*time.Second)
	defer cancel()
	if lines <= 0 {
		lines = 80
	}
	cmd := fmt.Sprintf("journalctl -u auto-update.service -n %d --no-pager -o short-iso", lines)
	if invocation != "" && isHex(invocation) {
		cmd = fmt.Sprintf("journalctl _SYSTEMD_INVOCATION_ID=%s -n %d --no-pager -o short-iso", invocation, lines)
	}
	out, _, err := Exec(ctx, dest, cmd)
	return strings.TrimSpace(out), err
}

func isHex(s string) bool {
	for _, c := range s {
		if !strings.ContainsRune("0123456789abcdef", c) {
			return false
		}
	}
	return s != ""
}
