// Command devenv-mcp is a Model Context Protocol server that orchestrates
// Nix-based development environments in rootless Podman containers for AI
// agents. It provides tools for environment lifecycle management, file
// operations, and command execution inside isolated containers.
package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"path"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

type srv struct {
	registry *EnvironmentRegistry
	jobs     *JobRegistry
	provider ContainerProvider

	// pollBase is the first gap between env_wait probes, overridden in tests.
	pollBase time.Duration
}

func main() {
	log.SetFlags(0)
	log.SetPrefix("devenv-mcp: ")

	cfg := parseFlags(os.Args[1:])
	s := newSrv(cfg)

	// Recover any environments that survived a server restart.
	if err := s.registry.Reconcile(context.Background()); err != nil {
		log.Printf("warning: reconciliation failed: %v", err)
	}

	if err := newMCPServer(s).Run(context.Background(), &mcp.StdioTransport{}); err != nil {
		log.Fatal(err)
	}
}

// newSrv wires a tool server onto a Podman-backed registry.
func newSrv(cfg config) *srv {
	provider := &PodmanProvider{PodmanPath: cfg.podmanPath}
	return &srv{
		registry: NewEnvironmentRegistry(provider, cfg.image, cfg.workDir, cfg.maxEnvs),
		jobs:     NewJobRegistry(provider, defaultMaxJobs),
		provider: provider,
		pollBase: defaultPollBase,
	}
}

// newMCPServer registers every tool the server exposes.
func newMCPServer(s *srv) *mcp.Server {
	server := mcp.NewServer(&mcp.Implementation{Name: "devenv-mcp", Version: "0.1.0"}, nil)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "env_create",
		Description: "Create a new development environment. Clones a git repo and/or injects a Nix devenv template into a rootless Podman container. Returns immediately with status 'creating'; poll env_list to check when it becomes 'ready'.",
	}, s.envCreate)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "env_list",
		Description: "List all development environments and their current status.",
	}, s.envList)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "env_destroy",
		Description: "Stop and remove a development environment and its container.",
	}, s.envDestroy)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "env_exec",
		Description: "Run a shell command inside a development environment. Blocks until the command completes or times out. Set background=true for long-running commands (dev servers, watchers, slow builds): it returns a job_id immediately, and the job is then observed with env_wait, env_job_output, and env_job_kill.",
	}, s.envExec)

	mcp.AddTool(server, &mcp.Tool{
		Name: "env_wait",
		Description: "Block until a condition holds inside an environment, instead of polling in a loop. Conditions: " +
			"env_ready (bootstrap finished), job_exited (a background job from env_exec finished), " +
			"command_succeeds (a shell command exits 0), port_open (something is listening on a TCP port), " +
			"file_exists (a path appears), file_matches (a file's tail matches a regex — use this to wait for a line in a log). " +
			"Returns as soon as the condition is met, or reports status 'timeout' without failing.",
	}, s.envWait)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "env_job_list",
		Description: "List background jobs, optionally filtered to one environment, with their status and exit codes.",
	}, s.envJobList)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "env_job_output",
		Description: "Read the output captured from a background job. Works while the job is still running.",
	}, s.envJobOutput)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "env_job_kill",
		Description: "Stop a background job: SIGTERM, then SIGKILL after a grace period. Set force=true to SIGKILL immediately.",
	}, s.envJobKill)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "env_read_file",
		Description: "Read the contents of a file inside a development environment.",
	}, s.envReadFile)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "env_write_file",
		Description: "Write content to a file inside a development environment. Creates parent directories as needed.",
	}, s.envWriteFile)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "env_list_dir",
		Description: "List the contents of a directory inside a development environment.",
	}, s.envListDir)

	mcp.AddTool(server, &mcp.Tool{
		Name:        "env_list_templates",
		Description: "List available devenv templates and well-known repository shortcuts.",
	}, s.envListTemplates)

	return server
}

// ---- flag parsing ----------------------------------------------------------

type config struct {
	podmanPath string
	image      string
	workDir    string
	maxEnvs    int
}

func parseFlags(args []string) config {
	cfg := config{
		image:   "docker.io/nixos/nix:latest",
		workDir: "/workspace",
		maxEnvs: 10,
	}
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--podman-path":
			i++
			if i < len(args) {
				cfg.podmanPath = args[i]
			}
		case "--image":
			i++
			if i < len(args) {
				cfg.image = args[i]
			}
		case "--work-dir":
			i++
			if i < len(args) {
				cfg.workDir = args[i]
			}
		case "--max-envs":
			i++
			if i < len(args) {
				n := 0
				for _, c := range args[i] {
					if c >= '0' && c <= '9' {
						n = n*10 + int(c-'0')
					}
				}
				if n > 0 {
					cfg.maxEnvs = n
				}
			}
		}
	}
	return cfg
}

// ---- tool input/output types -----------------------------------------------

type envCreateInput struct {
	Template string `json:"template,omitempty" jsonschema:"template name (go, rust, python, node-pnpm, node-yarn, node-npm) or 'auto' to detect from repo"`
	GitURL   string `json:"git_url,omitempty" jsonschema:"git URL to clone into the environment"`
	Name     string `json:"name,omitempty" jsonschema:"human-readable name (auto-generated if omitted)"`
}

type envCreateOutput struct {
	EnvID    string `json:"env_id"`
	Name     string `json:"name"`
	Status   string `json:"status"`
	Template string `json:"template,omitempty"`
	RepoURL  string `json:"repo_url,omitempty"`
}

type envIDInput struct {
	EnvID string `json:"env_id" jsonschema:"environment ID from env_create or env_list"`
}

type envExecInput struct {
	EnvID      string `json:"env_id" jsonschema:"environment ID"`
	Command    string `json:"command" jsonschema:"shell command to run"`
	WorkDir    string `json:"work_dir,omitempty" jsonschema:"working directory inside the container (defaults to workspace root)"`
	TimeoutSec int    `json:"timeout_sec,omitempty" jsonschema:"max seconds to wait (default 300); ignored when background is true"`
	Background bool   `json:"background,omitempty" jsonschema:"run the command in the background and return a job_id immediately"`
}

type envExecOutput struct {
	EnvID      string  `json:"env_id"`
	Command    string  `json:"command"`
	Status     string  `json:"status"`
	ExitCode   *int    `json:"exit_code,omitempty"`
	Output     string  `json:"output"`
	ElapsedSec float64 `json:"elapsed_sec"`
	Error      string  `json:"error,omitempty"`
	JobID      string  `json:"job_id,omitempty"`
}

type envWaitInput struct {
	EnvID      string `json:"env_id" jsonschema:"environment ID"`
	Condition  string `json:"condition" jsonschema:"one of env_ready, job_exited, command_succeeds, port_open, file_exists, file_matches"`
	JobID      string `json:"job_id,omitempty" jsonschema:"job ID to wait on (condition job_exited)"`
	Command    string `json:"command,omitempty" jsonschema:"shell command to poll until it exits 0 (condition command_succeeds)"`
	WorkDir    string `json:"work_dir,omitempty" jsonschema:"working directory for the probe command (condition command_succeeds)"`
	Port       int    `json:"port,omitempty" jsonschema:"TCP port to poll (condition port_open)"`
	Path       string `json:"path,omitempty" jsonschema:"file path to poll (conditions file_exists and file_matches)"`
	Pattern    string `json:"pattern,omitempty" jsonschema:"regular expression matched against the file tail (condition file_matches)"`
	TimeoutSec int    `json:"timeout_sec,omitempty" jsonschema:"max seconds to wait (default 300, max 3600)"`
}

type envWaitOutput struct {
	EnvID     string  `json:"env_id"`
	Condition string  `json:"condition"`
	Met       bool    `json:"met"`
	Status    string  `json:"status"`
	WaitedSec float64 `json:"waited_sec"`
	Detail    string  `json:"detail,omitempty"`
	JobID     string  `json:"job_id,omitempty"`
	ExitCode  *int    `json:"exit_code,omitempty"`
}

type jobIDInput struct {
	JobID string `json:"job_id" jsonschema:"job ID from env_exec with background=true"`
}

type envJobListInput struct {
	EnvID string `json:"env_id,omitempty" jsonschema:"only list jobs from this environment (default: all environments)"`
}

type envJobOutputInput struct {
	JobID string `json:"job_id" jsonschema:"job ID from env_exec with background=true"`
	Tail  int    `json:"tail,omitempty" jsonschema:"return only the last N lines (default: everything captured)"`
}

type envJobKillInput struct {
	JobID string `json:"job_id" jsonschema:"job ID from env_exec with background=true"`
	Force bool   `json:"force,omitempty" jsonschema:"send SIGKILL immediately instead of SIGTERM first"`
}

type jobInfoOutput struct {
	JobID      string  `json:"job_id"`
	EnvID      string  `json:"env_id"`
	Command    string  `json:"command"`
	Status     string  `json:"status"`
	ExitCode   *int    `json:"exit_code,omitempty"`
	ElapsedSec float64 `json:"elapsed_sec"`
	StartedAt  string  `json:"started_at"`
	Error      string  `json:"error,omitempty"`
}

type envJobListOutput struct {
	Jobs []jobInfoOutput `json:"jobs"`
}

type envJobOutputOutput struct {
	Job    jobInfoOutput `json:"job"`
	Output string        `json:"output"`
}

type envFileInput struct {
	EnvID string `json:"env_id" jsonschema:"environment ID"`
	Path  string `json:"path" jsonschema:"file path inside the container"`
}

type envWriteFileInput struct {
	EnvID   string `json:"env_id" jsonschema:"environment ID"`
	Path    string `json:"path" jsonschema:"file path inside the container"`
	Content string `json:"content" jsonschema:"file content to write"`
}

type envInfoOutput struct {
	EnvID     string `json:"env_id"`
	Name      string `json:"name"`
	Status    string `json:"status"`
	Template  string `json:"template,omitempty"`
	RepoURL   string `json:"repo_url,omitempty"`
	WorkDir   string `json:"work_dir"`
	CreatedAt string `json:"created_at"`
	Error     string `json:"error,omitempty"`
}

type envListOutput struct {
	Environments []envInfoOutput `json:"environments"`
}

// ---- tool handlers ---------------------------------------------------------

func (s *srv) envCreate(_ context.Context, _ *mcp.CallToolRequest, in envCreateInput) (*mcp.CallToolResult, envCreateOutput, error) {
	// Handle well-known repo shortcuts.
	gitURL := in.GitURL
	tmpl := in.Template
	if url, ok := wellKnownRepos[in.Template]; ok {
		gitURL = url
		tmpl = in.Template // mark as well-known
	}

	if gitURL == "" && tmpl == "" {
		return nil, envCreateOutput{}, fmt.Errorf("at least one of git_url or template is required")
	}

	// Validate up front: bootstrap runs asynchronously, so an unknown template
	// would otherwise surface as a failed environment minutes later.
	if err := validateTemplate(tmpl, gitURL); err != nil {
		return nil, envCreateOutput{}, err
	}

	env, err := s.registry.Create(context.Background(), in.Name, gitURL, tmpl)
	if err != nil {
		return nil, envCreateOutput{}, err
	}

	out := envCreateOutput{
		EnvID:    env.ID,
		Name:     env.Name,
		Status:   string(env.Status()),
		Template: env.Template(),
		RepoURL:  env.RepoURL,
	}
	return textResult(fmt.Sprintf("created environment %s (%s) — status: %s", out.EnvID, out.Name, out.Status)), out, nil
}

func (s *srv) envList(_ context.Context, _ *mcp.CallToolRequest, _ struct{}) (*mcp.CallToolResult, envListOutput, error) {
	envs := s.registry.List()
	sort.Slice(envs, func(i, j int) bool { return envs[i].ID < envs[j].ID })

	out := envListOutput{}
	for _, e := range envs {
		info := envInfoOutput{
			EnvID:     e.ID,
			Name:      e.Name,
			Status:    string(e.Status()),
			Template:  e.Template(),
			RepoURL:   e.RepoURL,
			WorkDir:   e.WorkDir,
			CreatedAt: e.CreatedAt.Format(time.RFC3339),
			Error:     e.Error(),
		}
		out.Environments = append(out.Environments, info)
	}

	return textResult(renderEnvList(out)), out, nil
}

func (s *srv) envDestroy(_ context.Context, _ *mcp.CallToolRequest, in envIDInput) (*mcp.CallToolResult, struct{}, error) {
	// Kill jobs first: the container is about to go away, and a job's podman
	// client would otherwise linger until it noticed.
	s.jobs.KillEnv(context.Background(), in.EnvID)

	if err := s.registry.Destroy(context.Background(), in.EnvID); err != nil {
		return nil, struct{}{}, err
	}
	return textResult(fmt.Sprintf("destroyed environment %s", in.EnvID)), struct{}{}, nil
}

func (s *srv) envExec(ctx context.Context, _ *mcp.CallToolRequest, in envExecInput) (*mcp.CallToolResult, envExecOutput, error) {
	env, ok := s.registry.Get(in.EnvID)
	if !ok {
		return nil, envExecOutput{}, fmt.Errorf("unknown environment %q", in.EnvID)
	}

	timeout := 300 * time.Second
	if in.TimeoutSec > 0 {
		timeout = time.Duration(in.TimeoutSec) * time.Second
	}

	workDir := env.WorkDir
	if in.WorkDir != "" {
		resolved, err := resolveWithin(env.WorkDir, in.WorkDir)
		if err != nil {
			return nil, envExecOutput{}, fmt.Errorf("work_dir: %w", err)
		}
		workDir = resolved
	}

	if in.Background {
		job, err := s.jobs.Start(env, in.Command, workDir)
		if err != nil {
			return nil, envExecOutput{}, err
		}
		out := envExecOutput{
			EnvID:   in.EnvID,
			Command: in.Command,
			Status:  string(job.Status()),
			JobID:   job.ID,
		}
		return textResult(fmt.Sprintf("started %s in %s: %s\nwait for it with env_wait (condition job_exited, port_open, or file_matches); read output with env_job_output",
			job.ID, in.EnvID, in.Command)), out, nil
	}

	cctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	start := time.Now()
	code, output, err := s.provider.Exec(cctx, env.ContainerID,
		[]string{"sh", "-c", in.Command},
		ExecOpts{WorkDir: workDir})

	res := envExecOutput{
		EnvID:      in.EnvID,
		Command:    in.Command,
		Output:     output,
		ElapsedSec: time.Since(start).Seconds(),
	}

	// Check the context first: on timeout the provider kills the child, which
	// surfaces as an ordinary non-zero exit rather than an error.
	switch {
	case cctx.Err() == context.DeadlineExceeded:
		res.Status = "timeout"
		res.Error = fmt.Sprintf("timed out after %s", timeout)
	case err != nil:
		res.Status = "error"
		res.Error = err.Error()
	default:
		res.Status = "exited"
		res.ExitCode = &code
	}

	return textResult(renderExec(res)), res, nil
}

func (s *srv) envReadFile(_ context.Context, _ *mcp.CallToolRequest, in envFileInput) (*mcp.CallToolResult, struct{}, error) {
	env, ok := s.registry.Get(in.EnvID)
	if !ok {
		return nil, struct{}{}, fmt.Errorf("unknown environment %q", in.EnvID)
	}

	target, err := resolveWithin(env.WorkDir, in.Path)
	if err != nil {
		return nil, struct{}{}, err
	}

	content, err := s.provider.ReadFile(context.Background(), env.ContainerID, target)
	if err != nil {
		return nil, struct{}{}, err
	}
	return textResult(string(content)), struct{}{}, nil
}

func (s *srv) envWriteFile(_ context.Context, _ *mcp.CallToolRequest, in envWriteFileInput) (*mcp.CallToolResult, struct{}, error) {
	env, ok := s.registry.Get(in.EnvID)
	if !ok {
		return nil, struct{}{}, fmt.Errorf("unknown environment %q", in.EnvID)
	}

	target, err := resolveWithin(env.WorkDir, in.Path)
	if err != nil {
		return nil, struct{}{}, err
	}

	if err := s.provider.WriteFile(context.Background(), env.ContainerID, target, []byte(in.Content)); err != nil {
		return nil, struct{}{}, err
	}
	return textResult(fmt.Sprintf("wrote %d bytes to %s", len(in.Content), in.Path)), struct{}{}, nil
}

func (s *srv) envListDir(_ context.Context, _ *mcp.CallToolRequest, in envFileInput) (*mcp.CallToolResult, struct{}, error) {
	env, ok := s.registry.Get(in.EnvID)
	if !ok {
		return nil, struct{}{}, fmt.Errorf("unknown environment %q", in.EnvID)
	}

	target, err := resolveWithin(env.WorkDir, in.Path)
	if err != nil {
		return nil, struct{}{}, err
	}

	code, output, err := s.provider.Exec(context.Background(), env.ContainerID,
		[]string{"ls", "-la", target}, ExecOpts{})
	if err != nil || code != 0 {
		msg := output
		if err != nil {
			msg = err.Error()
		}
		return nil, struct{}{}, fmt.Errorf("ls failed: %s", msg)
	}
	return textResult(output), struct{}{}, nil
}

func (s *srv) envListTemplates(_ context.Context, _ *mcp.CallToolRequest, _ struct{}) (*mcp.CallToolResult, struct{}, error) {
	var b strings.Builder
	b.WriteString("built-in templates:\n")
	for _, t := range builtinTemplates {
		fmt.Fprintf(&b, "  %-12s %s (indicators: %s)\n", t.Name, t.Description, strings.Join(t.Indicators, ", "))
	}
	if len(wellKnownRepos) > 0 {
		b.WriteString("\nwell-known repositories:\n")
		for name, url := range wellKnownRepos {
			fmt.Fprintf(&b, "  %-20s %s\n", name, url)
		}
	}
	return textResult(b.String()), struct{}{}, nil
}

func (s *srv) envWait(ctx context.Context, _ *mcp.CallToolRequest, in envWaitInput) (*mcp.CallToolResult, envWaitOutput, error) {
	env, ok := s.registry.Get(in.EnvID)
	if !ok {
		return nil, envWaitOutput{}, fmt.Errorf("unknown environment %q", in.EnvID)
	}

	timeout := defaultWaitTimeout
	if in.TimeoutSec > 0 {
		timeout = time.Duration(in.TimeoutSec) * time.Second
		if timeout > maxWaitTimeout {
			timeout = maxWaitTimeout
		}
	}

	out := envWaitOutput{EnvID: in.EnvID, Condition: in.Condition}
	var (
		res waitResult
		err error
		// unsatisfiable marks a condition that resolved into a state it can
		// never reach, so the agent stops retrying instead of waiting again.
		unsatisfiable bool
	)

	switch in.Condition {
	case condEnvReady:
		res, err = waitForClose(ctx, env.buildCh, timeout)
		if err == nil && res.met {
			res.detail = "status=" + string(env.Status())
			if msg := env.Error(); msg != "" {
				res.detail += " error=" + strconv.Quote(msg)
			}
			if env.Status() != StatusReady {
				res.met, unsatisfiable = false, true
			}
		}

	case condJobExited:
		var job *Job
		job, err = s.requireJob(in.JobID, in.EnvID)
		if err != nil {
			break
		}
		out.JobID = job.ID
		res, err = waitForClose(ctx, job.Done(), timeout)
		if err == nil && res.met {
			out.ExitCode = job.ExitCode()
			res.detail = "status=" + string(job.Status())
			if msg := job.Error(); msg != "" {
				res.detail += " error=" + strconv.Quote(msg)
			}
		}

	case condCommandSucceeds:
		if in.Command == "" {
			err = fmt.Errorf("condition %s requires command", condCommandSucceeds)
			break
		}
		workDir := env.WorkDir
		if in.WorkDir != "" {
			if workDir, err = resolveWithin(env.WorkDir, in.WorkDir); err != nil {
				err = fmt.Errorf("work_dir: %w", err)
				break
			}
		}
		res, err = waitFor(ctx, commandProbe(s.provider, env.ContainerID, in.Command, workDir), timeout, s.pollBase)

	case condPortOpen:
		if in.Port < 1 || in.Port > 65535 {
			err = fmt.Errorf("condition %s requires port between 1 and 65535, got %d", condPortOpen, in.Port)
			break
		}
		res, err = waitFor(ctx, portOpenProbe(s.provider, env.ContainerID, in.Port), timeout, s.pollBase)

	case condFileExists:
		var target string
		if target, err = resolveWithin(env.WorkDir, in.Path); err != nil {
			break
		}
		res, err = waitFor(ctx, fileExistsProbe(s.provider, env.ContainerID, target), timeout, s.pollBase)

	case condFileMatches:
		var target string
		if target, err = resolveWithin(env.WorkDir, in.Path); err != nil {
			break
		}
		if in.Pattern == "" {
			err = fmt.Errorf("condition %s requires pattern", condFileMatches)
			break
		}
		var re *regexp.Regexp
		if re, err = regexp.Compile(in.Pattern); err != nil {
			err = fmt.Errorf("pattern: %w", err)
			break
		}
		res, err = waitFor(ctx, fileMatchesProbe(s.provider, env.ContainerID, target, re), timeout, s.pollBase)

	default:
		err = fmt.Errorf("unknown condition %q (available: %s)", in.Condition, strings.Join(waitConditions, ", "))
	}

	if err != nil {
		return nil, envWaitOutput{}, err
	}

	out.Met, out.Detail, out.WaitedSec = res.met, res.detail, res.waitedSec
	switch {
	case unsatisfiable:
		out.Status = "error"
	case res.met:
		out.Status = "met"
	default:
		out.Status = "timeout"
	}
	return textResult(renderWait(out)), out, nil
}

// requireJob looks up a job and confirms it belongs to the given environment,
// so a stale job_id cannot be waited on against the wrong container.
func (s *srv) requireJob(jobID, envID string) (*Job, error) {
	if jobID == "" {
		return nil, fmt.Errorf("job_id is required")
	}
	job, ok := s.jobs.Get(jobID)
	if !ok {
		return nil, fmt.Errorf("unknown job %q", jobID)
	}
	if envID != "" && job.EnvID != envID {
		return nil, fmt.Errorf("job %s belongs to %s, not %s", jobID, job.EnvID, envID)
	}
	return job, nil
}

func (s *srv) envJobList(_ context.Context, _ *mcp.CallToolRequest, in envJobListInput) (*mcp.CallToolResult, envJobListOutput, error) {
	jobs := s.jobs.List(in.EnvID)
	sort.Slice(jobs, func(i, j int) bool { return jobs[i].ID < jobs[j].ID })

	out := envJobListOutput{}
	for _, j := range jobs {
		out.Jobs = append(out.Jobs, jobInfo(j))
	}
	return textResult(renderJobList(out)), out, nil
}

func (s *srv) envJobOutput(_ context.Context, _ *mcp.CallToolRequest, in envJobOutputInput) (*mcp.CallToolResult, envJobOutputOutput, error) {
	job, err := s.requireJob(in.JobID, "")
	if err != nil {
		return nil, envJobOutputOutput{}, err
	}

	out := envJobOutputOutput{Job: jobInfo(job), Output: tailLines(job.Output(), in.Tail)}
	return textResult(renderJobInfo(out.Job) + "\n" + out.Output), out, nil
}

func (s *srv) envJobKill(ctx context.Context, _ *mcp.CallToolRequest, in envJobKillInput) (*mcp.CallToolResult, jobInfoOutput, error) {
	if _, err := s.requireJob(in.JobID, ""); err != nil {
		return nil, jobInfoOutput{}, err
	}

	job, err := s.jobs.Kill(ctx, in.JobID, in.Force)
	if err != nil {
		return nil, jobInfoOutput{}, err
	}
	out := jobInfo(job)
	return textResult(renderJobInfo(out)), out, nil
}

// ---- helpers ---------------------------------------------------------------

// validateTemplate rejects template names that no built-in, well-known repo,
// or auto-detection can satisfy.
func validateTemplate(tmpl, gitURL string) error {
	if tmpl == "" {
		return nil
	}
	if _, ok := wellKnownRepos[tmpl]; ok {
		return nil
	}
	if tmpl == "auto" {
		if gitURL == "" {
			return fmt.Errorf("template \"auto\" requires git_url: there is nothing to detect from")
		}
		return nil
	}
	if templateByName(tmpl) != nil {
		return nil
	}
	names := make([]string, 0, len(builtinTemplates)+1)
	for _, t := range builtinTemplates {
		names = append(names, t.Name)
	}
	names = append(names, "auto")
	return fmt.Errorf("unknown template %q (available: %s)", tmpl, strings.Join(names, ", "))
}

func textResult(s string) *mcp.CallToolResult {
	return &mcp.CallToolResult{Content: []mcp.Content{&mcp.TextContent{Text: s}}}
}

// resolveWithin resolves p against the workspace root and rejects anything
// that escapes it. Relative paths are taken from the root; the result is
// cleaned first, so traversal via ".." is caught rather than passing a naive
// prefix test. Paths are container paths, so they are always slash-separated
// regardless of the host OS.
func resolveWithin(workDir, p string) (string, error) {
	if p == "" {
		return "", fmt.Errorf("path is required")
	}
	if !strings.HasPrefix(p, "/") {
		p = workDir + "/" + p
	}
	clean := path.Clean(p)
	root := path.Clean(workDir)
	if clean != root && !strings.HasPrefix(clean, root+"/") {
		return "", fmt.Errorf("path must be under %s", root)
	}
	return clean, nil
}

func renderEnvList(o envListOutput) string {
	if len(o.Environments) == 0 {
		return "no environments"
	}
	var b strings.Builder
	for _, e := range o.Environments {
		fmt.Fprintf(&b, "%s name=%s status=%s", e.EnvID, e.Name, e.Status)
		if e.Template != "" {
			fmt.Fprintf(&b, " template=%s", e.Template)
		}
		if e.RepoURL != "" {
			fmt.Fprintf(&b, " repo=%s", e.RepoURL)
		}
		if e.Error != "" {
			fmt.Fprintf(&b, " error=%q", e.Error)
		}
		b.WriteString("\n")
	}
	return strings.TrimRight(b.String(), "\n")
}

// jobInfo projects a job into its tool-facing shape.
func jobInfo(j *Job) jobInfoOutput {
	return jobInfoOutput{
		JobID:      j.ID,
		EnvID:      j.EnvID,
		Command:    j.Command,
		Status:     string(j.Status()),
		ExitCode:   j.ExitCode(),
		ElapsedSec: j.ElapsedSec(),
		StartedAt:  j.StartedAt.Format(time.RFC3339),
		Error:      j.Error(),
	}
}

func renderWait(o envWaitOutput) string {
	var b strings.Builder
	fmt.Fprintf(&b, "env=%s condition=%s status=%s met=%t waited=%.1fs",
		o.EnvID, o.Condition, o.Status, o.Met, o.WaitedSec)
	if o.JobID != "" {
		fmt.Fprintf(&b, " job=%s", o.JobID)
	}
	if o.ExitCode != nil {
		fmt.Fprintf(&b, " exit=%d", *o.ExitCode)
	}
	if o.Detail != "" {
		fmt.Fprintf(&b, "\n%s", o.Detail)
	}
	return b.String()
}

func renderJobInfo(o jobInfoOutput) string {
	var b strings.Builder
	fmt.Fprintf(&b, "%s env=%s status=%s", o.JobID, o.EnvID, o.Status)
	if o.ExitCode != nil {
		fmt.Fprintf(&b, " exit=%d", *o.ExitCode)
	}
	fmt.Fprintf(&b, " elapsed=%.1fs", o.ElapsedSec)
	if o.Error != "" {
		fmt.Fprintf(&b, " error=%q", o.Error)
	}
	fmt.Fprintf(&b, " command=%q", o.Command)
	return b.String()
}

func renderJobList(o envJobListOutput) string {
	if len(o.Jobs) == 0 {
		return "no jobs"
	}
	var b strings.Builder
	for _, j := range o.Jobs {
		b.WriteString(renderJobInfo(j))
		b.WriteString("\n")
	}
	return strings.TrimRight(b.String(), "\n")
}

func renderExec(o envExecOutput) string {
	var b strings.Builder
	fmt.Fprintf(&b, "env=%s status=%s", o.EnvID, o.Status)
	if o.ExitCode != nil {
		fmt.Fprintf(&b, " exit=%d", *o.ExitCode)
	}
	fmt.Fprintf(&b, " elapsed=%.1fs\n", o.ElapsedSec)
	if o.Error != "" {
		fmt.Fprintf(&b, "error: %s\n", o.Error)
	}
	b.WriteString(o.Output)
	return b.String()
}
