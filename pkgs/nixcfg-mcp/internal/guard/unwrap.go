package guard

import (
	"strings"
)

// unwrap looks through a launcher to the command it runs. It returns either the
// inner argv, or a script string to parse again (bash -c, ssh host 'cmd',
// eval). ok is false when c is not a launcher, or launches nothing inspectable.
func unwrap(c call) (inner []string, script string, remote bool, ok bool) {
	args := c.args
	remote = c.remote
	switch c.name() {
	case "sudo", "doas":
		rest := skipOpts(args[1:], "ugphCDRTU", []string{"user", "group", "host", "prompt", "close-from", "chdir", "role", "type", "other-user"})
		return rest, "", remote, len(rest) > 0
	case "env":
		rest := args[1:]
		for len(rest) > 0 {
			a := rest[0]
			switch {
			case a == "--":
				rest = rest[1:]
				return rest, "", remote, len(rest) > 0
			case a == "-S" || a == "--split-string":
				if len(rest) > 1 {
					return nil, strings.Join(rest[1:], " "), remote, true
				}
				return nil, "", remote, false
			case strings.HasPrefix(a, "--split-string="):
				return nil, strings.TrimPrefix(a, "--split-string=") + " " + strings.Join(rest[1:], " "), remote, true
			case a == "-u" || a == "-C" || a == "--unset" || a == "--chdir":
				rest = rest[min(2, len(rest)):]
			case strings.HasPrefix(a, "-"):
				rest = rest[1:]
			case strings.Contains(a, "="):
				rest = rest[1:]
			default:
				return rest, "", remote, true
			}
		}
		return nil, "", remote, false
	case "command", "exec", "builtin", "nohup", "time", "lowprio", "unbuffer", "setsid", "chronic":
		rest := skipOpts(args[1:], "", nil)
		return rest, "", remote, len(rest) > 0
	case "nice", "renice":
		rest := skipOpts(args[1:], "n", []string{"adjustment"})
		return rest, "", remote, len(rest) > 0
	case "ionice":
		rest := skipOpts(args[1:], "cnp", []string{"class", "classdata", "pid"})
		return rest, "", remote, len(rest) > 0
	case "stdbuf":
		rest := skipOpts(args[1:], "ioe", []string{"input", "output", "error"})
		return rest, "", remote, len(rest) > 0
	case "timeout":
		rest := skipOpts(args[1:], "sk", []string{"signal", "kill-after"})
		if len(rest) > 1 { // drop the duration
			return rest[1:], "", remote, true
		}
		return nil, "", remote, false
	case "flock":
		rest := skipOpts(args[1:], "wEc", []string{"timeout", "conflict-exit-code", "command"})
		for i, a := range args {
			if (a == "-c" || a == "--command") && i+1 < len(args) {
				return nil, args[i+1], remote, true
			}
		}
		if len(rest) > 1 { // drop the lock file
			return rest[1:], "", remote, true
		}
		return nil, "", remote, false
	case "xargs":
		rest := skipOpts(args[1:], "IiLlnPsdEa", []string{"replace", "max-lines", "max-args", "max-procs", "max-chars", "delimiter", "eof", "arg-file", "process-slot-var"})
		return rest, "", remote, len(rest) > 0
	case "watch":
		rest := skipOpts(args[1:], "nqd", []string{"interval", "differences", "chgexit"})
		if len(rest) > 0 {
			return nil, strings.Join(rest, " "), remote, true
		}
		return nil, "", remote, false
	case "systemd-run", "systemd-inhibit", "run0":
		rest := skipOpts(args[1:], "puEMHsd", []string{"property", "unit", "setenv", "machine", "host", "slice", "description", "what", "who", "why", "mode", "uid", "gid", "nice", "working-directory", "on-active", "on-calendar", "timer-property", "path-property", "socket-property", "service-type", "background"})
		return rest, "", remote, len(rest) > 0
	case "bash", "sh", "zsh", "dash", "fish", "ksh":
		for i := 1; i < len(args); i++ {
			a := args[i]
			if a == "--" || !strings.HasPrefix(a, "-") {
				break
			}
			if !strings.HasPrefix(a, "--") && strings.Contains(a, "c") {
				if i+1 < len(args) {
					return nil, args[i+1], remote, true
				}
				return nil, "", remote, false
			}
		}
		return nil, "", remote, false
	case "eval":
		if len(args) > 1 {
			return nil, strings.Join(args[1:], " "), remote, true
		}
		return nil, "", remote, false
	case "nix-shell":
		for i, a := range args {
			if (a == "--run" || a == "--command") && i+1 < len(args) {
				return nil, args[i+1], remote, true
			}
		}
		return nil, "", remote, false
	case "nix":
		if len(args) > 1 && (args[1] == "develop" || args[1] == "shell") {
			for i, a := range args {
				if (a == "-c" || a == "--command") && i+1 < len(args) {
					return args[i+1:], "", remote, true
				}
			}
		}
		return nil, "", remote, false
	case "ssh", "mosh", "tailscale":
		if c.name() == "tailscale" {
			if len(args) < 2 || args[1] != "ssh" {
				return nil, "", remote, false
			}
			args = args[1:]
		}
		rest := skipOpts(args[1:], "BbcDEeFIiJLlmOopQRSWw", nil)
		if len(rest) > 1 { // rest[0] is the destination
			return nil, strings.Join(rest[1:], " "), true, true
		}
		return nil, "", remote, false
	}
	return nil, "", remote, false
}

// skipOpts drops leading options. valued lists the short flags that take a
// value (as the next word when not attached) and long lists the long options
// that take one (as the next word when written without "=").
func skipOpts(args []string, valued string, long []string) []string {
	for i := 0; i < len(args); i++ {
		a := args[i]
		switch {
		case a == "--":
			return args[i+1:]
		case strings.HasPrefix(a, "--"):
			name, _, hasVal := strings.Cut(a[2:], "=")
			if !hasVal && contains(long, name) {
				i++
			}
		case strings.HasPrefix(a, "-") && len(a) > 1:
			// A short cluster: the first valued flag takes the rest of the
			// cluster, or the next word when it is last.
			for j := 1; j < len(a); j++ {
				if strings.IndexByte(valued, a[j]) >= 0 {
					if j == len(a)-1 {
						i++
					}
					break
				}
			}
		default:
			return args[i:]
		}
	}
	return nil
}

func contains(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}
