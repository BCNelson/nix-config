// Package forgejo reads CI state from the Forgejo instance. Everything it needs
// is readable without authentication (the repository is public); FORGEJO_TOKEN
// is sent when set, for rate limits or a private mirror.
package forgejo

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"regexp"
	"strconv"
	"strings"
	"time"
)

// Client talks to one repository.
type Client struct {
	Base string // https://git.bcnelson.dev
	Repo string // owner/name
	HTTP *http.Client
}

func New(base, repo string) *Client {
	return &Client{Base: strings.TrimRight(base, "/"), Repo: repo, HTTP: &http.Client{Timeout: 30 * time.Second}}
}

func (c *Client) get(ctx context.Context, url string, v any) error {
	ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	if tok := os.Getenv("FORGEJO_TOKEN"); tok != "" {
		req.Header.Set("Authorization", "token "+tok)
	}
	req.Header.Set("Accept", "application/json")
	resp, err := c.HTTP.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return fmt.Errorf("GET %s: %s: %s", url, resp.Status, strings.TrimSpace(string(b)))
	}
	return json.NewDecoder(resp.Body).Decode(v)
}

func (c *Client) api(path string) string {
	return fmt.Sprintf("%s/api/v1/repos/%s%s", c.Base, c.Repo, path)
}

// Status is one commit status context, e.g. "Nix Flake Check / check-hosts (push)".
type Status struct {
	Context     string    `json:"context"`
	State       string    `json:"status"` // pending | success | failure | error | warning
	TargetURL   string    `json:"target_url"`
	Description string    `json:"description"`
	CreatedAt   time.Time `json:"created_at"`
	UpdatedAt   time.Time `json:"updated_at"`
}

// Job extracts the job name from the context ("check-hosts").
func (s Status) Job() string {
	_, rest, ok := strings.Cut(s.Context, " / ")
	if !ok {
		rest = s.Context
	}
	if i := strings.LastIndex(rest, " ("); i >= 0 {
		rest = rest[:i]
	}
	return rest
}

// Event is the trigger in parentheses ("push", "pull_request").
func (s Status) Event() string {
	i := strings.LastIndex(s.Context, " (")
	if i < 0 {
		return ""
	}
	return strings.TrimSuffix(s.Context[i+2:], ")")
}

// Workflow is the workflow name before " / ".
func (s Status) Workflow() string {
	w, _, _ := strings.Cut(s.Context, " / ")
	return w
}

var targetRe = regexp.MustCompile(`/actions/runs/(\d+)/jobs/(\d+)`)

// RunJob returns the run index and job index from the target URL.
func (s Status) RunJob() (run, job int, ok bool) {
	m := targetRe.FindStringSubmatch(s.TargetURL)
	if m == nil {
		return 0, 0, false
	}
	run, _ = strconv.Atoi(m[1])
	job, _ = strconv.Atoi(m[2])
	return run, job, true
}

// CombinedStatus is a commit's statuses.
type CombinedStatus struct {
	State    string   `json:"state"`
	Statuses []Status `json:"statuses"`
}

// CommitStatus returns the statuses of a commit (latest per context).
func (c *Client) CommitStatus(ctx context.Context, sha string) (*CombinedStatus, error) {
	var cs CombinedStatus
	if err := c.get(ctx, c.api("/commits/"+sha+"/status"), &cs); err != nil {
		return nil, err
	}
	return &cs, nil
}

// Run is one workflow run.
type Run struct {
	ID         int64     `json:"id"`
	Index      int       `json:"index_in_repo"`
	WorkflowID string    `json:"workflow_id"`
	Status     string    `json:"status"` // waiting | running | success | failure | cancelled | skipped | blocked
	CommitSHA  string    `json:"commit_sha"`
	Event      string    `json:"event"`
	PrettyRef  string    `json:"prettyref"`
	Started    time.Time `json:"started"`
	Stopped    time.Time `json:"stopped"`
	HTMLURL    string    `json:"html_url"`
}

// Done reports whether the run has finished.
func (r Run) Done() bool {
	switch r.Status {
	case "success", "failure", "cancelled", "skipped":
		return true
	}
	return false
}

// RunsForSHA lists the workflow runs whose head is sha.
func (c *Client) RunsForSHA(ctx context.Context, sha string) ([]Run, error) {
	var resp struct {
		Runs []Run `json:"workflow_runs"`
	}
	if err := c.get(ctx, c.api("/actions/runs?head_sha="+sha), &resp); err != nil {
		return nil, err
	}
	return resp.Runs, nil
}

// RecentRuns lists recent runs of every workflow and event (the API ignores
// limit and returns them all, newest first).
func (c *Client) RecentRuns(ctx context.Context) ([]Run, error) {
	var resp struct {
		Runs []Run `json:"workflow_runs"`
	}
	if err := c.get(ctx, c.api("/actions/runs?limit=50"), &resp); err != nil {
		return nil, err
	}
	return resp.Runs, nil
}

// RunJob is one job of a run.
type RunJob struct {
	ID      int64  `json:"id"`
	Name    string `json:"name"`
	Attempt int    `json:"attempt"`
	Status  string `json:"status"`
}

// Jobs lists the jobs of a run (by run id, not index), in order.
func (c *Client) Jobs(ctx context.Context, runID int64) ([]RunJob, error) {
	var jobs []RunJob
	if err := c.get(ctx, c.api(fmt.Sprintf("/actions/runs/%d/jobs", runID)), &jobs); err != nil {
		return nil, err
	}
	return jobs, nil
}

// LogURL is the plain-text log of a job attempt (an undocumented web route).
func (c *Client) LogURL(runIndex, jobIndex, attempt int) string {
	if attempt < 1 {
		attempt = 1
	}
	return fmt.Sprintf("%s/%s/actions/runs/%d/jobs/%d/attempt/%d/logs", c.Base, c.Repo, runIndex, jobIndex, attempt)
}

// WebURL is the human-facing page for a run.
func (c *Client) WebURL(runIndex int) string {
	return fmt.Sprintf("%s/%s/actions/runs/%d", c.Base, c.Repo, runIndex)
}

// Log fetches a job log. Logs run to megabytes; it is streamed through fn line
// by line rather than held in memory.
func (c *Client) Log(ctx context.Context, runIndex, jobIndex, attempt int, fn func(line string)) error {
	ctx, cancel := context.WithTimeout(ctx, 2*time.Minute)
	defer cancel()
	url := c.LogURL(runIndex, jobIndex, attempt)
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	if tok := os.Getenv("FORGEJO_TOKEN"); tok != "" {
		req.Header.Set("Authorization", "token "+tok)
	}
	resp, err := (&http.Client{}).Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("GET %s: %s", url, resp.Status)
	}
	sc := bufio.NewScanner(resp.Body)
	sc.Buffer(make([]byte, 64*1024), 4*1024*1024)
	for sc.Scan() {
		fn(sc.Text())
	}
	return sc.Err()
}
