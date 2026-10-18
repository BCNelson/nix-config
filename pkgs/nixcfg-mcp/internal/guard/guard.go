// Package guard decides whether an agent's tool call should be blocked.
//
// It enforces the repo's human-only boundaries (see AGENTS.md "Guardrails"):
// commits need an explicit pathspec, secrets are never read, key-touch and
// deploy-control commands are handed to the human, and nothing activates a
// system. Shell commands are parsed (mvdan.cc/sh) rather than pattern-matched,
// so only words in command position count: `echo "just rekey"` is fine,
// `bash -c 'just rekey'` is not.
//
// The guard is a seatbelt, not a sandbox. A command assembled at run time
// (`$GIT commit`, a script written to disk and run) gets past it; that is an
// accepted limitation, not something to work around.
package guard

import (
	"path/filepath"
	"strings"

	"mvdan.cc/sh/v3/syntax"
)

// RulesVersion is bumped whenever the rules change, so the shim can warn when
// an installed binary lags the repo (.agents/guard-version).
const RulesVersion = "2"

// Decision is the verdict on one tool call.
type Decision struct {
	Block  bool   `json:"block"`
	Rule   string `json:"rule,omitempty"`
	Reason string `json:"reason,omitempty"`
	// Human is the one-line command to hand the human instead, if any.
	Human string `json:"human_command,omitempty"`
}

var allow = Decision{}

func block(rule, reason, human string) Decision {
	return Decision{Block: true, Rule: rule, Reason: reason, Human: human}
}

// Message renders a decision for an agent to read.
func (d Decision) Message() string {
	if !d.Block {
		return ""
	}
	var b strings.Builder
	b.WriteString("Blocked by the nix-config guard (")
	b.WriteString(d.Rule)
	b.WriteString("): ")
	b.WriteString(d.Reason)
	if d.Human != "" {
		b.WriteString("\nHand this to the human, on one line: ")
		b.WriteString(d.Human)
	}
	b.WriteString("\nSee AGENTS.md \"Guardrails\". Don't look for another way round this.")
	return b.String()
}

// CheckCommand inspects a shell command line.
func CheckCommand(cmd string) Decision {
	return checkScript(cmd, false, 0)
}

// CheckArgv inspects a command given as an argument vector (Codex sends
// shell calls this way).
func CheckArgv(argv []string) Decision {
	return checkCall(call{args: argv}, 0)
}

// CheckReadPath inspects a path a file-reading tool is about to open.
func CheckReadPath(p string) Decision {
	if isSensitivePath(p) {
		return block("secret-read",
			"this is a git-crypt secrets file (plaintext in this checkout). Its values are never needed to edit Nix that uses them: read the code that calls libx.getSecret instead.",
			"")
	}
	if strings.HasPrefix(filepath.Clean(p), "/run/agenix") {
		return block("secret-read", "decrypted agenix secrets are never read by agents.", "")
	}
	return allow
}

const maxDepth = 6

func checkScript(src string, remote bool, depth int) Decision {
	if depth > maxDepth {
		return allow
	}
	f, err := syntax.NewParser(syntax.Variant(syntax.LangBash)).Parse(strings.NewReader(src), "")
	if err != nil {
		// Unparseable: fall back to a coarse word split so an obvious
		// `git commit` in a broken one-liner is still caught.
		return checkCall(call{args: strings.Fields(src), remote: remote}, depth)
	}
	var d Decision
	syntax.Walk(f, func(n syntax.Node) bool {
		if d.Block {
			return false
		}
		switch x := n.(type) {
		case *syntax.CallExpr:
			c := call{remote: remote}
			for _, w := range x.Args {
				s, _ := wordString(w)
				c.args = append(c.args, s)
			}
			if len(c.args) > 0 {
				d = checkCall(c, depth)
			}
		case *syntax.Stmt:
			for _, r := range x.Redirs {
				if r.Word == nil {
					continue
				}
				target, _ := wordString(r.Word)
				if dd := checkRedirect(r.Op, target); dd.Block {
					d = dd
				}
			}
		}
		return true
	})
	return d
}

// wordString flattens a shell word to the string it would expand to when it is
// literal. Expansions are kept as their source text so they never collide with
// a real command name; the bool reports whether the word was fully literal.
func wordString(w *syntax.Word) (string, bool) {
	var b strings.Builder
	literal := true
	var walk func(parts []syntax.WordPart)
	walk = func(parts []syntax.WordPart) {
		for _, p := range parts {
			switch x := p.(type) {
			case *syntax.Lit:
				b.WriteString(x.Value)
			case *syntax.SglQuoted:
				b.WriteString(x.Value)
			case *syntax.DblQuoted:
				walk(x.Parts)
			default:
				literal = false
				b.WriteString("\x00")
			}
		}
	}
	walk(w.Parts)
	return b.String(), literal
}

func checkRedirect(op syntax.RedirOperator, target string) Decision {
	switch op {
	case syntax.RdrIn, syntax.RdrInOut:
		if isSensitivePath(target) {
			return secretFileBlock()
		}
		if strings.HasPrefix(target, "/run/agenix") {
			return agenixRunBlock()
		}
	case syntax.RdrOut, syntax.AppOut, syntax.RdrAll, syntax.AppAll, syntax.RdrClob:
		if strings.Contains(target, autoUpdateState) {
			return stateWriteBlock()
		}
	}
	return allow
}

// call is one simple command after redirections are stripped.
type call struct {
	args   []string
	remote bool // ran through ssh on another machine
}

func (c call) name() string {
	if len(c.args) == 0 {
		return ""
	}
	return filepath.Base(c.args[0])
}

// checkCall unwraps launchers (sudo, env, bash -c, ssh host ...) and applies
// the rules to the command underneath.
func checkCall(c call, depth int) Decision {
	if depth > maxDepth || len(c.args) == 0 {
		return allow
	}
	if inner, script, remote, ok := unwrap(c); ok {
		if script != "" {
			return checkScript(script, remote, depth+1)
		}
		return checkCall(call{args: inner, remote: remote}, depth+1)
	}
	for _, rule := range rules {
		if d := rule(c); d.Block {
			return d
		}
	}
	return allow
}
