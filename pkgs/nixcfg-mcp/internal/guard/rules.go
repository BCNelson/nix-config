package guard

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"

	"mvdan.cc/sh/v3/syntax"
)

const autoUpdateState = "/var/lib/auto-update/state"

var rules = []func(call) Decision{
	ruleGit,
	ruleSecretTools,
	ruleSecretArgs,
	ruleRecursiveSearch,
	ruleKeyTouch,
	ruleDeployControls,
	ruleActivation,
}

// ---- helpers ----------------------------------------------------------------

// quoteArgs renders argv back into a one-line shell command.
func quoteArgs(args []string) string {
	out := make([]string, 0, len(args))
	for _, a := range args {
		if strings.ContainsRune(a, '\x00') {
			return ""
		}
		q, err := syntax.Quote(a, syntax.LangBash)
		if err != nil {
			return ""
		}
		out = append(out, q)
	}
	return strings.Join(out, " ")
}

// humanFor is the command to hand the human: the agent's own command when it is
// a plain local one, else nothing (the reason says what to do).
func humanFor(c call) string {
	if c.remote {
		return ""
	}
	return quoteArgs(c.args)
}

// positional returns the non-option words after skipping options with
// skipOpts' conventions; the first is usually the subcommand.
func positional(args []string, valued string, long []string) []string {
	var out []string
	for i := 0; i < len(args); i++ {
		a := args[i]
		switch {
		case a == "--":
			return append(out, args[i+1:]...)
		case strings.HasPrefix(a, "--"):
			name, _, hasVal := strings.Cut(a[2:], "=")
			if !hasVal && contains(long, name) {
				i++
			}
		case strings.HasPrefix(a, "-") && len(a) > 1:
			for j := 1; j < len(a); j++ {
				if strings.IndexByte(valued, a[j]) >= 0 {
					if j == len(a)-1 {
						i++
					}
					break
				}
			}
		default:
			out = append(out, a)
		}
	}
	return out
}

func hasFlag(args []string, short byte, long ...string) bool {
	for _, a := range args {
		if a == "--" {
			return false
		}
		if strings.HasPrefix(a, "--") {
			name, _, _ := strings.Cut(a[2:], "=")
			if contains(long, name) {
				return true
			}
			continue
		}
		if short != 0 && strings.HasPrefix(a, "-") && len(a) > 1 && strings.IndexByte(a[1:], short) >= 0 {
			return true
		}
	}
	return false
}

// gitSub splits `git [global options] <sub> <args>`.
func gitSub(c call) (string, []string, bool) {
	if c.name() != "git" {
		return "", nil, false
	}
	args := c.args[1:]
	for i := 0; i < len(args); i++ {
		a := args[i]
		switch {
		case a == "-C" || a == "-c" || a == "--git-dir" || a == "--work-tree" || a == "--namespace" || a == "--config-env":
			i++
		case strings.HasPrefix(a, "-"):
		default:
			return a, args[i+1:], true
		}
	}
	return "", nil, false
}

// ---- sensitive files -------------------------------------------------------

func isSensitivePath(p string) bool {
	if p == "" {
		return false
	}
	// rev:path (git show), --file=path, -Ipath and similar.
	for _, sep := range []string{":", "="} {
		if _, after, ok := strings.Cut(p, sep); ok && isSensitivePath(after) {
			return true
		}
	}
	return filepath.Base(p) == "sensitive.nix"
}

var (
	sensitiveOnce  sync.Once
	sensitiveFiles []string // absolute paths of tracked sensitive.nix files
)

// knownSensitive lists the git-crypt files of the repository containing the
// working directory (empty outside a repo).
func knownSensitive() []string {
	sensitiveOnce.Do(func() {
		top, err := exec.Command("git", "rev-parse", "--show-toplevel").Output()
		if err != nil {
			return
		}
		root := strings.TrimSpace(string(top))
		out, err := exec.Command("git", "-C", root, "ls-files", "-z", "--", ":(glob)**/sensitive.nix").Output()
		if err != nil {
			return
		}
		for _, f := range strings.Split(string(out), "\x00") {
			if f != "" {
				sensitiveFiles = append(sensitiveFiles, filepath.Join(root, f))
			}
		}
	})
	return sensitiveFiles
}

// coversSensitive reports whether reading dir recursively would reach a
// sensitive file.
func coversSensitive(dir string) bool {
	abs, err := filepath.Abs(dir)
	if err != nil {
		return false
	}
	for _, f := range knownSensitive() {
		if f == abs || strings.HasPrefix(f, abs+string(os.PathSeparator)) {
			return true
		}
	}
	return false
}

// globHitsSensitive reports whether a glob-looking argument matches a sensitive file.
func globHitsSensitive(arg string) bool {
	if !strings.ContainsAny(arg, "*?[") {
		return false
	}
	matches, _ := filepath.Glob(arg)
	for _, m := range matches {
		if isSensitivePath(m) {
			return true
		}
	}
	return false
}

func secretFileBlock() Decision {
	return block("secret-read",
		"that touches a git-crypt secrets file (**/sensitive.nix), which is plaintext in this checkout. Agents never read these; work from the code that calls libx.getSecret, and ask the human for anything you need to know about a value.",
		"")
}

func agenixRunBlock() Decision {
	return block("secret-read", "/run/agenix holds decrypted secrets; agents never read them. Check a secret exists with `ls -l` or `nixcfg secrets-status <host>`.", "")
}

func stateWriteBlock() Decision {
	return block("deploy-control",
		"the auto-update state file is changed only by the human (it controls retries on every host).",
		"nixcfg human reset-retries <host>")
}

// ---- rules -------------------------------------------------------------------

// ruleGit: pathspec-only commits, and pushing is the human's.
func ruleGit(c call) Decision {
	sub, rest, ok := gitSub(c)
	if !ok {
		return allow
	}
	switch sub {
	case "commit":
		return checkCommit(rest)
	case "push":
		return checkPush(rest)
	case "crypt":
		return checkGitCrypt(append([]string{"git-crypt"}, rest...), c)
	}
	return allow
}

func checkCommit(args []string) Decision {
	var paths, all, interactive, patch, dryRun, fromFile bool
	for i := 0; i < len(args); i++ {
		a := args[i]
		if a == "--" {
			paths = paths || i+1 < len(args)
			break
		}
		if strings.HasPrefix(a, "--") {
			name, _, hasVal := strings.Cut(a[2:], "=")
			switch name {
			case "all":
				all = true
			case "interactive":
				interactive = true
			case "patch":
				patch = true
			case "dry-run":
				dryRun = true
			case "pathspec-from-file":
				fromFile = true
				if !hasVal {
					i++
				}
			case "message", "file", "reuse-message", "reedit-message", "author", "date",
				"cleanup", "template", "fixup", "squash", "trailer":
				if !hasVal {
					i++
				}
			}
			continue
		}
		if strings.HasPrefix(a, "-") && len(a) > 1 {
		cluster:
			for j := 1; j < len(a); j++ {
				switch a[j] {
				case 'a':
					all = true
				case 'p':
					patch = true
				case 'm', 'F', 'C', 'c', 't':
					if j == len(a)-1 {
						i++
					}
					break cluster
				case 'S', 'u':
					break cluster
				}
			}
			continue
		}
		paths = true
	}
	const how = "commit with an explicit pathspec: git commit -m \"msg\" -- <path> <path>. This tree usually has unrelated staged work in progress, and anything committed to main deploys."
	switch {
	case dryRun:
		return allow
	case all:
		return block("commit-pathspec", "`git commit -a` sweeps in every modified tracked file; "+how, "")
	case interactive || patch:
		return block("commit-pathspec", "interactive commits can't be driven by an agent; "+how, "")
	case !paths && !fromFile:
		return block("commit-pathspec", "a bare `git commit` takes everything staged; "+how, "")
	}
	return allow
}

// checkPush allows ordinary pushes (main deploys through CI, which is the
// point) but not the two ways round the gate: rewriting history, and writing
// the auto-update branch directly (hosts build it with no check-hosts run).
func checkPush(args []string) Decision {
	for _, a := range args {
		if a == "--" {
			break
		}
		if a == "-f" || a == "--force" || strings.HasPrefix(a, "--force-with-lease") || strings.HasPrefix(a, "--force-if-includes") ||
			a == "--mirror" || a == "-d" || a == "--delete" || a == "--prune" ||
			(strings.HasPrefix(a, "-") && !strings.HasPrefix(a, "--") && strings.ContainsRune(a, 'f')) {
			return block("push-force",
				"force-pushing, deleting or mirroring refs rewrites what hosts and CI see; push normally (fast-forward) instead.",
				"")
		}
	}
	for _, a := range positional(args, "o", []string{"push-option", "repo", "receive-pack", "exec", "recurse-submodules"}) {
		// A refspec like main:auto-update, +x:auto-update or refs/heads/auto-update.
		dst := a
		if _, after, ok := strings.Cut(a, ":"); ok {
			dst = after
		}
		dst = strings.TrimPrefix(strings.TrimPrefix(dst, "+"), "refs/heads/")
		if dst == "auto-update" {
			return block("deploy-control",
				"auto-update is advanced only by CI, after check-hosts passes (forward only). Push to main and let CI move it.",
				"")
		}
		if strings.HasPrefix(a, "+") {
			return block("push-force", "a +refspec is a force push; push normally (fast-forward) instead.", "")
		}
	}
	return allow
}

func checkGitCrypt(args []string, c call) Decision {
	sub := positional(args[1:], "k", []string{"key-name"})
	if len(sub) == 0 {
		return allow
	}
	switch sub[0] {
	case "unlock", "export-key":
		return block("key-touch", "git-crypt unlock and key export need the human's GPG / hardware key.", "just unlock")
	case "lock":
		return block("key-touch", "locking git-crypt changes what every evaluation sees; the human does this.", "just lock")
	}
	return allow
}

// ruleSecretTools: decrypting tools, whatever their arguments.
func ruleSecretTools(c call) Decision {
	args := c.args
	switch c.name() {
	case "git-crypt":
		return checkGitCrypt(args, c)
	case "age", "rage":
		for _, a := range args[1:] {
			if a == "--decrypt" || (strings.HasPrefix(a, "-") && !strings.HasPrefix(a, "--") && strings.Contains(a, "d")) {
				return block("secret-read", "decrypting age files needs the human's hardware key and would put plaintext in your context.", "")
			}
		}
	case "agenix":
		for _, a := range args[1:] {
			if a == "-e" || a == "-d" || a == "--edit" || a == "--decrypt" {
				return block("secret-read", "editing or decrypting a secret is the human's job (it needs their hardware key).", humanFor(c))
			}
		}
		pos := positional(args[1:], "i", []string{"identity"})
		if len(pos) > 0 {
			switch pos[0] {
			case "edit", "view", "decrypt":
				return block("secret-read", "editing or viewing a secret is the human's job (it needs their hardware key and shows plaintext).", humanFor(c))
			case "rekey", "update-masterkeys":
				return block("key-touch", "rekeying decrypts every master secret with the human's hardware key.", "just rekey")
			}
		}
	case "gpg", "gpg2":
		for _, a := range args[1:] {
			if a == "-d" || a == "--decrypt" || strings.HasSuffix(a, "local.key.asc") || a == "--export-secret-keys" {
				return block("secret-read", "decrypting with GPG needs the human's key.", "")
			}
		}
	case "bw":
		if pos := positional(args[1:], "", []string{"session"}); len(pos) > 0 {
			switch pos[0] {
			case "get", "unlock", "export", "list", "login", "serve":
				return block("secret-read", "Bitwarden holds the plaintext secrets; agents don't read it.", "")
			}
		}
	case "systemd-creds":
		if pos := positional(args[1:], "", nil); len(pos) > 0 && pos[0] == "decrypt" {
			return block("secret-read", "decrypting credentials is not something agents do.", "")
		}
	}
	return allow
}

// readOnlyMeta are commands that only look at a file's name or metadata.
var readOnlyMeta = map[string]bool{
	"ls": true, "stat": true, "file": true, "wc": true, "du": true, "test": true, "[": true,
	"realpath": true, "readlink": true, "basename": true, "dirname": true,
}

// ruleSecretArgs: any other command naming a sensitive file, or /run/agenix.
func ruleSecretArgs(c call) Decision {
	name := c.name()
	args := c.args[1:]

	if readOnlyMeta[name] {
		return allow
	}
	if sub, rest, ok := gitSub(c); ok {
		switch sub {
		case "add", "status", "ls-files", "check-attr", "check-ignore", "rm", "mv", "commit", "restore", "checkout":
			return allow
		case "log":
			if !hasFlag(rest, 'p', "patch") && !hasFlag(rest, 'u', "unified") {
				return allow
			}
		case "diff", "show":
			if hasFlag(rest, 0, "stat", "name-only", "name-status", "numstat", "shortstat") {
				return allow
			}
		}
		args = rest
	}
	if name == "nix" || name == "nix-instantiate" || name == "nix-build" {
		for _, a := range args {
			if strings.Contains(a, "sensitive") {
				return secretFileBlock()
			}
		}
	}

	// grep-likes: the first positional word is a pattern, not a file.
	skipPattern := (name == "grep" || name == "egrep" || name == "fgrep" || name == "rg" || name == "ag") &&
		!hasFlag(args, 'e', "regexp") && !hasFlag(args, 'f', "file")
	for _, a := range args {
		if strings.HasPrefix(a, "-") && !strings.Contains(a, "=") {
			continue
		}
		if skipPattern {
			skipPattern = false
			continue
		}
		if strings.HasPrefix(a, ":!") || strings.HasPrefix(a, ":^") || strings.HasPrefix(a, ":(exclude)") {
			continue // a git pathspec that leaves the file out
		}
		if isSensitivePath(a) || globHitsSensitive(a) {
			return secretFileBlock()
		}
		if strings.HasPrefix(a, "/run/agenix") && name != "ls" {
			return agenixRunBlock()
		}
		if strings.Contains(a, autoUpdateState) && writesFile(name, args) {
			return stateWriteBlock()
		}
	}
	return allow
}

func writesFile(name string, args []string) bool {
	switch name {
	case "cat", "less", "more", "head", "tail", "grep", "rg", "awk", "stat", "ls", "wc", "file", "jq":
		return false
	case "sed", "perl":
		return hasFlag(args, 'i', "in-place")
	}
	return true
}

// ruleRecursiveSearch: recursive searches that would read sensitive.nix.
// ripgrep (and so Claude's Grep tool) honours .ignore, which excludes them.
func ruleRecursiveSearch(c call) Decision {
	if c.remote {
		return allow
	}
	name := c.name()
	args := c.args[1:]
	excludes := false
	for _, a := range args {
		if strings.Contains(a, "sensitive") && (strings.HasPrefix(a, "--exclude") || strings.HasPrefix(a, "-g") ||
			strings.HasPrefix(a, "--glob") || strings.HasPrefix(a, "--iglob") || strings.HasPrefix(a, ":!") || strings.HasPrefix(a, ":(exclude)") || strings.HasPrefix(a, ":^")) {
			excludes = true
		}
	}
	if excludes {
		return allow
	}
	var targets []string
	switch {
	case name == "grep" || name == "egrep" || name == "fgrep":
		if !hasFlag(args, 'r', "recursive", "dereference-recursive") && !hasFlag(args, 'R') {
			return allow
		}
		pos := positional(args, "efmABCdD", []string{"regexp", "file", "max-count", "after-context", "before-context", "context", "directories", "devices", "include", "exclude", "exclude-dir", "label", "color", "colour", "binary-files"})
		if !hasFlag(args, 'e', "regexp") && !hasFlag(args, 'f', "file") && len(pos) > 0 {
			pos = pos[1:]
		}
		targets = pos
		if len(targets) == 0 {
			targets = []string{"."}
		}
	case name == "rg":
		noIgnore := hasFlag(args, 0, "no-ignore", "no-ignore-dot", "no-ignore-files")
		for _, a := range args {
			if strings.HasPrefix(a, "-u") && strings.Trim(a[1:], "u") == "" {
				noIgnore = true
			}
		}
		if !noIgnore {
			return allow
		}
		targets = []string{"."}
	case name == "git":
		sub, rest, _ := gitSub(c)
		if sub != "grep" || hasFlag(rest, 0, "cached") {
			return allow
		}
		targets = []string{"."}
	case name == "find":
		for _, a := range args {
			if a == "-exec" || a == "-execdir" {
				if len(args) > 0 && !strings.HasPrefix(args[0], "-") {
					targets = []string{args[0]}
				} else {
					targets = []string{"."}
				}
				break
			}
		}
	default:
		return allow
	}
	for _, t := range targets {
		if coversSensitive(t) {
			return block("secret-read",
				"that recursive search would read git-crypt secrets (**/sensitive.nix, plaintext in this checkout). Use rg without --no-ignore/-u (it honours .ignore), or exclude them: grep -r --exclude=sensitive.nix ...",
				"")
		}
	}
	return allow
}

var keyTouchRecipes = map[string]string{
	"rekey":             "rekeying decrypts every master secret with the human's hardware key.",
	"new-secrets":       "this rekeys too, which needs the human's hardware key. Run `just generate-secrets` yourself, then hand over `just rekey`.",
	"sync-secrets":      "syncing to Bitwarden decrypts every secret with the human's key and Bitwarden password.",
	"unlock":            "git-crypt unlock needs the human's GPG key.",
	"lock":              "locking git-crypt changes what every evaluation sees; the human does this.",
	"push":              "this reads a GitHub token from sensitive.nix; push with `git push origin main` instead.",
	"pull":              "this reads a token from sensitive.nix; the human runs it.",
	"sync":              "this pulls with a token from sensitive.nix and switches this machine; the human runs it.",
	"tofu":              "OpenTofu reads state credentials from sensitive.nix and changes real DNS; the human runs it.",
	"terraform":         "OpenTofu reads state credentials from sensitive.nix and changes real DNS; the human runs it.",
	"update-os":         "activating a system is the human's call.",
	"apply":             "activating a system is the human's call.",
	"os":                "activating a system is the human's call.",
	"o":                 "activating a system is the human's call.",
	"all":               "activating a system is the human's call.",
	"update-home":       "activating a home-manager generation is the human's call.",
	"home":              "activating a home-manager generation is the human's call.",
	"h":                 "activating a home-manager generation is the human's call.",
	"rollback":          "activating a system is the human's call.",
	"setup":             "activating a system is the human's call.",
	"isoInstall":        "this writes to a USB drive; the human runs it.",
	"bootstrap-selinux": "this changes the host's SELinux policy with sudo; the human runs it.",
}

// ruleKeyTouch: just recipes, hardware-key tools, OpenTofu.
func ruleKeyTouch(c call) Decision {
	args := c.args[1:]
	switch c.name() {
	case "just":
		pos := positional(args, "fd", []string{"justfile", "working-directory", "set", "shell", "shell-arg", "dotenv-filename", "dotenv-path", "color", "command-color", "dump-format", "list-heading", "list-prefix", "chooser"})
		if len(pos) == 0 {
			return allow
		}
		if why, ok := keyTouchRecipes[pos[0]]; ok {
			rule := "key-touch"
			if strings.HasPrefix(why, "activating") || strings.Contains(why, "switches") {
				rule = "activation"
			}
			human := humanFor(c)
			if pos[0] == "push" {
				human = "" // the agent can push itself, with git
			}
			return block(rule, why, human)
		}
	case "ykman", "age-plugin-yubikey", "age-plugin-fido2-hmac":
		return block("key-touch", "hardware-key tools are the human's.", humanFor(c))
	case "nix":
		if len(args) > 0 && args[0] == "run" {
			for _, a := range args {
				if strings.Contains(a, "age-bitwarden-sync") {
					return block("key-touch", "syncing to Bitwarden decrypts every secret with the human's key.", "just sync-secrets")
				}
			}
		}
	case "tofu", "terraform":
		pos := positional(args, "", []string{"chdir", "var", "var-file", "target"})
		if len(pos) > 0 {
			switch pos[0] {
			case "apply", "destroy", "import", "state", "taint", "untaint", "force-unlock", "refresh":
				return block("human-only", "OpenTofu changes real DNS and state; the human runs it, via `just tofu`.", "just tofu "+quoteArgs(c.args[1:]))
			}
		}
	}
	return allow
}

// ruleDeployControls: ntfy, the Forgejo API, auto-update on hosts.
func ruleDeployControls(c call) Decision {
	args := c.args[1:]
	switch c.name() {
	case "curl", "wget", "http", "https", "xh", "curlie", "httpie":
		mutating := false
		for i, a := range args {
			if strings.Contains(a, "ntfy.sh") {
				return block("deploy-control", "an ntfy ping wakes auto-update on every subscribed host; CI sends it, or the human does.", "")
			}
			if a == "-d" || strings.HasPrefix(a, "--data") || a == "-F" || a == "--form" || a == "--json" || a == "-T" || a == "--upload-file" ||
				strings.HasPrefix(a, "--post") || strings.HasPrefix(a, "--method") ||
				((a == "-X" || a == "--request") && i+1 < len(args) && !strings.EqualFold(args[i+1], "GET") && !strings.EqualFold(args[i+1], "HEAD")) ||
				(strings.HasPrefix(a, "-X") && len(a) > 2 && !strings.EqualFold(a[2:], "GET")) {
				mutating = true
			}
		}
		if mutating {
			for _, a := range args {
				if strings.Contains(a, "git.bcnelson.dev") {
					return block("deploy-control", "changing things through the Forgejo API (re-running CI, editing refs) is the human's call; reading it is fine.", "")
				}
			}
		}
	case "ntfy":
		if pos := positional(args, "", nil); len(pos) > 0 {
			switch pos[0] {
			case "publish", "pub", "send", "trigger":
				return block("deploy-control", "an ntfy ping wakes auto-update on every subscribed host; CI sends it, or the human does.", "")
			}
		}
	case "systemctl":
		pos := positional(args, "pPtHMnoS", []string{"property", "type", "state", "host", "machine", "lines", "output", "signal", "kill-whom", "job-mode", "root", "image", "what", "timestamp", "kill-value", "message", "when", "boot-loader-menu", "boot-loader-entry", "reboot-argument"})
		if len(pos) == 0 {
			return allow
		}
		switch pos[0] {
		case "reboot", "poweroff", "halt", "kexec", "soft-reboot", "suspend", "hibernate", "rescue", "emergency", "isolate", "switch-root":
			return block("activation", "rebooting or changing a machine's state is the human's call.", humanFor(c))
		case "start", "restart", "try-restart", "reload-or-restart", "try-reload-or-restart", "stop", "kill", "enable", "disable", "mask", "unmask", "reload":
			for _, u := range pos[1:] {
				if strings.HasPrefix(u, "auto-update") || strings.HasPrefix(u, "autoupdate") {
					return block("deploy-control",
						"starting or stopping auto-update is the human's call (and stop doesn't pause it: the ntfy subscriber restarts it within seconds).",
						"nixcfg human kick <host>")
				}
			}
		}
	case "reboot", "shutdown", "poweroff", "halt", "kexec":
		return block("activation", "rebooting a machine is the human's call.", humanFor(c))
	}
	return allow
}

// ruleActivation: switching a system or home generation.
func ruleActivation(c call) Decision {
	args := c.args[1:]
	switch c.name() {
	case "nixos-rebuild", "nixos-rebuild-ng", "darwin-rebuild":
		pos := positional(args, "IpcjfAo", []string{"flake", "target-host", "build-host", "profile-name", "specialisation", "max-jobs", "cores", "builders", "file", "attr", "log-format", "include", "option", "override-input", "store", "eval-store"})
		for _, p := range pos {
			switch p {
			case "switch", "boot", "test":
				return block("activation", "activating a system is the human's call. Build it instead: just build-host <host> (or nixos-rebuild build).", humanFor(c))
			}
		}
	case "switch-to-configuration":
		return block("activation", "activating a system is the human's call.", "")
	case "home-manager":
		if pos := positional(args, "fbAI", []string{"flake", "file", "attr", "option"}); len(pos) > 0 && pos[0] == "switch" {
			return block("activation", "activating a home-manager generation is the human's call. Build it instead: home-manager build --flake .#<user>@<host>.", humanFor(c))
		}
	case "system-manager":
		if pos := positional(args, "", []string{"flake", "target-host"}); len(pos) > 0 && (pos[0] == "switch" || pos[0] == "activate" || pos[0] == "deactivate") {
			return block("activation", "activating a system is the human's call.", humanFor(c))
		}
	case "nix":
		joined := strings.Join(args, " ")
		if len(args) > 0 && args[0] == "run" && strings.Contains(joined, "system-manager") && strings.Contains(joined, "switch") {
			return block("activation", "activating a system is the human's call.", "")
		}
	case "nix-env":
		for _, a := range args {
			if strings.Contains(a, "/nix/var/nix/profiles/system") {
				return block("activation", "changing the system profile is the human's call.", "")
			}
		}
	}
	return allow
}
