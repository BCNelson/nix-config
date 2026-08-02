package main

import (
	"context"
	"fmt"
	"regexp"
	"strconv"
	"strings"
	"time"
)

// Wait conditions accepted by env_wait.
const (
	condEnvReady        = "env_ready"
	condJobExited       = "job_exited"
	condCommandSucceeds = "command_succeeds"
	condPortOpen        = "port_open"
	condFileExists      = "file_exists"
	condFileMatches     = "file_matches"
)

var waitConditions = []string{
	condEnvReady, condJobExited, condCommandSucceeds,
	condPortOpen, condFileExists, condFileMatches,
}

const (
	// defaultWaitTimeout applies when the caller does not set timeout_sec.
	defaultWaitTimeout = 300 * time.Second
	// maxWaitTimeout caps timeout_sec so a mistyped value cannot park an agent
	// for hours.
	maxWaitTimeout = 3600 * time.Second
	// defaultPollBase is the first gap between probes. It backs off up to
	// maxPollInterval so a long wait does not hammer the container runtime.
	defaultPollBase = 200 * time.Millisecond
	maxPollInterval = 2 * time.Second
	// maxProbeTimeout bounds a single probe, so one hung exec cannot consume
	// the whole wait.
	maxProbeTimeout = 30 * time.Second
	// fileMatchWindow is how much of a file's tail is searched by file_matches.
	fileMatchWindow = 128 * 1024
	// detailLimit truncates probe output quoted back in the result.
	detailLimit = 400
)

// probe reports whether a condition currently holds. detail describes what was
// observed either way; a returned error aborts the wait.
type probe func(ctx context.Context) (met bool, detail string, err error)

// waitResult is the outcome of a wait.
type waitResult struct {
	met       bool
	detail    string
	waitedSec float64
}

// waitFor polls until the condition holds, the timeout expires, or the probe
// fails. Between probes it backs off from pollBase up to maxPollInterval.
func waitFor(ctx context.Context, p probe, timeout, pollBase time.Duration) (waitResult, error) {
	start := time.Now()
	deadline := start.Add(timeout)
	if pollBase <= 0 {
		pollBase = defaultPollBase
	}
	interval := pollBase

	for {
		probeCtx, cancel := context.WithTimeout(ctx, probeTimeout(deadline))
		met, detail, err := p(probeCtx)
		cancel()

		if err != nil {
			return waitResult{detail: detail, waitedSec: time.Since(start).Seconds()}, err
		}
		if met {
			return waitResult{met: true, detail: detail, waitedSec: time.Since(start).Seconds()}, nil
		}

		remaining := time.Until(deadline)
		if remaining <= 0 {
			return waitResult{detail: detail, waitedSec: time.Since(start).Seconds()}, nil
		}
		if interval > remaining {
			interval = remaining
		}

		select {
		case <-time.After(interval):
		case <-ctx.Done():
			return waitResult{detail: detail, waitedSec: time.Since(start).Seconds()}, ctx.Err()
		}

		if interval = interval * 2; interval > maxPollInterval {
			interval = maxPollInterval
		}
	}
}

// probeTimeout gives a single probe the lesser of maxProbeTimeout and the time
// left, with a small floor so the last probe before the deadline still runs.
func probeTimeout(deadline time.Time) time.Duration {
	remaining := time.Until(deadline)
	if remaining < time.Second {
		remaining = time.Second
	}
	if remaining > maxProbeTimeout {
		return maxProbeTimeout
	}
	return remaining
}

// waitForClose waits on an already-signalling channel instead of polling. It is
// used for conditions the server observes directly (bootstrap and job exit).
func waitForClose(ctx context.Context, ch <-chan struct{}, timeout time.Duration) (waitResult, error) {
	start := time.Now()
	select {
	case <-ch:
		return waitResult{met: true, waitedSec: time.Since(start).Seconds()}, nil
	case <-time.After(timeout):
		return waitResult{waitedSec: time.Since(start).Seconds()}, nil
	case <-ctx.Done():
		return waitResult{waitedSec: time.Since(start).Seconds()}, ctx.Err()
	}
}

// ---- probes ----------------------------------------------------------------

// commandProbe reports success when the command exits 0 inside the container.
func commandProbe(provider ContainerProvider, cid ContainerID, command, workDir string) probe {
	return func(ctx context.Context) (bool, string, error) {
		code, output, err := provider.Exec(ctx, cid, []string{"sh", "-c", command}, ExecOpts{WorkDir: workDir})
		if err != nil {
			return false, "", fmt.Errorf("probe failed: %w", err)
		}
		detail := fmt.Sprintf("exit=%d", code)
		if trimmed := strings.TrimSpace(output); trimmed != "" {
			detail += " output=" + strconv.Quote(truncate(trimmed, detailLimit))
		}
		return code == 0, detail, nil
	}
}

// fileExistsProbe reports success once path exists inside the container.
func fileExistsProbe(provider ContainerProvider, cid ContainerID, path string) probe {
	return func(ctx context.Context) (bool, string, error) {
		code, _, err := provider.Exec(ctx, cid, []string{"test", "-e", path}, ExecOpts{})
		if err != nil {
			return false, "", fmt.Errorf("probe failed: %w", err)
		}
		if code == 0 {
			return true, path + " exists", nil
		}
		return false, path + " does not exist yet", nil
	}
}

// fileMatchesProbe reports success once the tail of path matches re. A missing
// or unreadable file is "not yet", not an error: the usual case is waiting for
// a log file that has not been created.
func fileMatchesProbe(provider ContainerProvider, cid ContainerID, path string, re *regexp.Regexp) probe {
	window := strconv.Itoa(fileMatchWindow)
	return func(ctx context.Context) (bool, string, error) {
		code, output, err := provider.Exec(ctx, cid,
			[]string{"tail", "-c", window, path}, ExecOpts{})
		if err != nil {
			return false, "", fmt.Errorf("probe failed: %w", err)
		}
		if code != 0 {
			return false, path + " is not readable yet", nil
		}
		if m := re.FindString(output); m != "" {
			return true, "matched " + strconv.Quote(truncate(m, detailLimit)), nil
		}
		return false, fmt.Sprintf("no match in the last %d bytes of %s", len(output), path), nil
	}
}

// portOpenProbe reports success once something is listening on port inside the
// container's network namespace. It reads /proc/net/tcp directly so it does not
// depend on nc, ss, or bash being present in the image.
func portOpenProbe(provider ContainerProvider, cid ContainerID, port int) probe {
	return func(ctx context.Context) (bool, string, error) {
		code, output, err := provider.Exec(ctx, cid,
			[]string{"sh", "-c", "cat /proc/net/tcp /proc/net/tcp6 2>/dev/null"}, ExecOpts{})
		if err != nil {
			return false, "", fmt.Errorf("probe failed: %w", err)
		}
		if code != 0 {
			return false, "", fmt.Errorf("cannot read /proc/net/tcp in the container")
		}
		if listeningOn(output, port) {
			return true, fmt.Sprintf("a process is listening on port %d", port), nil
		}
		return false, fmt.Sprintf("nothing is listening on port %d yet", port), nil
	}
}

// tcpStateListen is the /proc/net/tcp state code for LISTEN.
const tcpStateListen = "0A"

// listeningOn parses /proc/net/tcp{,6} content and reports whether any socket
// is in LISTEN on the given port. Lines look like:
//
//	sl  local_address rem_address st ...
//	0: 0100007F:1F90 00000000:0000 0A ...
//
// where both the address and the state are hex.
func listeningOn(procNet string, port int) bool {
	want := fmt.Sprintf("%04X", port)
	for _, line := range strings.Split(procNet, "\n") {
		fields := strings.Fields(line)
		if len(fields) < 4 || !strings.HasSuffix(fields[0], ":") {
			continue // header or short line
		}
		_, hexPort, ok := strings.Cut(fields[1], ":")
		if !ok || !strings.EqualFold(hexPort, want) {
			continue
		}
		if strings.EqualFold(fields[3], tcpStateListen) {
			return true
		}
	}
	return false
}

// truncate shortens s to n bytes, marking that it was cut.
func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}
