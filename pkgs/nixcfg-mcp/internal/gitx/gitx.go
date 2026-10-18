// Package gitx runs the handful of git queries nixcfg needs, each with a hard
// deadline so a stalled remote can never wedge a wait.
package gitx

import (
	"bytes"
	"context"
	"fmt"
	"os/exec"
	"regexp"
	"strings"
	"time"
)

// Repo is a local checkout.
type Repo struct {
	Dir string
	// Remotes to try in order for network operations ("origin", "github").
	Remotes []string
}

var shaRe = regexp.MustCompile(`^[0-9a-f]{7,40}$`)

// IsSHA reports whether s looks like a (possibly abbreviated) commit id.
func IsSHA(s string) bool { return shaRe.MatchString(s) }

func (r Repo) run(ctx context.Context, timeout time.Duration, args ...string) (string, error) {
	out, err := r.runRaw(ctx, timeout, args...)
	return strings.TrimSpace(out), err
}

// runRaw is run without trimming the output.
func (r Repo) runRaw(ctx context.Context, timeout time.Duration, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, "git", append([]string{"-C", r.Dir}, args...)...)
	cmd.Env = append(cmd.Environ(), "GIT_TERMINAL_PROMPT=0", "GIT_SSH_COMMAND=ssh -o BatchMode=yes -o ConnectTimeout=10")
	var out, errb bytes.Buffer
	cmd.Stdout = &out
	cmd.Stderr = &errb
	if err := cmd.Run(); err != nil {
		if ctx.Err() == context.DeadlineExceeded {
			return "", fmt.Errorf("git %s: timed out after %s", strings.Join(args, " "), timeout)
		}
		msg := strings.TrimSpace(errb.String())
		if msg == "" {
			msg = err.Error()
		}
		return "", fmt.Errorf("git %s: %s", args[0], msg)
	}
	return out.String(), nil
}

// RevParse resolves a revision to a full commit id.
func (r Repo) RevParse(ctx context.Context, rev string) (string, error) {
	return r.run(ctx, 10*time.Second, "rev-parse", "--verify", "--quiet", rev+"^{commit}")
}

// IsAncestor reports whether a is an ancestor of (or equal to) b. Both must be
// present locally.
func (r Repo) IsAncestor(ctx context.Context, a, b string) (bool, error) {
	ctx2, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx2, "git", "-C", r.Dir, "merge-base", "--is-ancestor", a, b)
	err := cmd.Run()
	if err == nil {
		return true, nil
	}
	if ee, ok := err.(*exec.ExitError); ok && ee.ExitCode() == 1 {
		return false, nil
	}
	return false, fmt.Errorf("git merge-base --is-ancestor %s %s: %v", short(a), short(b), err)
}

// Has reports whether a commit object exists locally.
func (r Repo) Has(ctx context.Context, sha string) bool {
	_, err := r.run(ctx, 5*time.Second, "cat-file", "-e", sha+"^{commit}")
	return err == nil
}

// Fetch updates the remote-tracking refs for branches from the first remote
// that answers. It returns the remote used.
func (r Repo) Fetch(ctx context.Context, branches ...string) (string, error) {
	var lastErr error
	for _, remote := range r.remotes() {
		args := []string{"fetch", "--quiet", "--no-tags", remote}
		for _, b := range branches {
			args = append(args, fmt.Sprintf("+refs/heads/%s:refs/remotes/%s/%s", b, remote, b))
		}
		if _, err := r.run(ctx, 120*time.Second, args...); err != nil {
			lastErr = err
			continue
		}
		return remote, nil
	}
	return "", lastErr
}

// LsRemote returns the commit a remote branch points at, trying each remote.
func (r Repo) LsRemote(ctx context.Context, branch string) (sha, remote string, err error) {
	var lastErr error
	for _, rem := range r.remotes() {
		out, err := r.run(ctx, 30*time.Second, "ls-remote", rem, "refs/heads/"+branch)
		if err != nil {
			lastErr = err
			continue
		}
		f := strings.Fields(out)
		if len(f) == 0 {
			return "", rem, nil
		}
		return f[0], rem, nil
	}
	return "", "", lastErr
}

// ChangedFiles lists the paths changed between two commits (a..b).
func (r Repo) ChangedFiles(ctx context.Context, a, b string) ([]string, error) {
	out, err := r.run(ctx, 30*time.Second, "diff", "--name-only", "--no-renames", a, b)
	if err != nil {
		return nil, err
	}
	return lines(out), nil
}

// CommitFiles lists the paths a single commit changed (against its first parent).
func (r Repo) CommitFiles(ctx context.Context, sha string) ([]string, error) {
	out, err := r.run(ctx, 30*time.Second, "diff-tree", "--no-commit-id", "--name-only", "-r", "--root", "-m", "--first-parent", sha)
	if err != nil {
		return nil, err
	}
	return lines(out), nil
}

// Commits lists commits in a..b, newest first.
func (r Repo) Commits(ctx context.Context, a, b string) ([]string, error) {
	rng := b
	if a != "" {
		rng = a + ".." + b
	}
	out, err := r.run(ctx, 30*time.Second, "rev-list", rng)
	if err != nil {
		return nil, err
	}
	return lines(out), nil
}

// WorkingChanges lists paths with uncommitted changes (staged, unstaged or untracked).
func (r Repo) WorkingChanges(ctx context.Context) ([]string, error) {
	out, err := r.runRaw(ctx, 20*time.Second, "status", "--porcelain=v1", "-z", "--untracked-files=all")
	if err != nil {
		return nil, err
	}
	var paths []string
	recs := strings.Split(out, "\x00")
	for i := 0; i < len(recs); i++ {
		rec := recs[i]
		if len(rec) < 4 {
			continue
		}
		paths = append(paths, rec[3:])
		if rec[0] == 'R' || rec[0] == 'C' {
			i++ // a rename's source path follows; skip it
		}
	}
	return paths, nil
}

// Format runs git log -1 --format=<format> on a commit.
func (r Repo) Format(ctx context.Context, sha, format string) (string, error) {
	return r.run(ctx, 5*time.Second, "log", "-1", "--format="+format, sha)
}

// Subject returns a commit's one-line subject.
func (r Repo) Subject(ctx context.Context, sha string) string {
	out, _ := r.run(ctx, 5*time.Second, "log", "-1", "--format=%s", sha)
	return out
}

func (r Repo) remotes() []string {
	if len(r.Remotes) > 0 {
		return r.Remotes
	}
	return []string{"origin", "github"}
}

func lines(s string) []string {
	var out []string
	for _, l := range strings.Split(s, "\n") {
		if l = strings.TrimSpace(l); l != "" {
			out = append(out, l)
		}
	}
	return out
}

func short(s string) string {
	if len(s) > 10 {
		return s[:10]
	}
	return s
}

// Short abbreviates a commit id for display.
func Short(s string) string { return short(s) }
