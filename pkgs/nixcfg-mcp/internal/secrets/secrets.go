// Package secrets compares a host's declared agenix-rekey secrets with what is
// in the tree, without decrypting anything. The expected rekeyed file name is
// computed the way agenix-rekey does (modules/agenix-rekey.nix,
// rekeyedLocalSecret):
//
//	sha256(sha256(hostPubkey) + sha256file(rekeyFile))[0:32] + "-" + name + ".age"
//
// so a missing file means the secret was never rekeyed for this host, or its
// master file changed since.
package secrets

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

// expr is applied to config.age. It reads only names, rekeyFile, generator and
// owner, so agenix-rekey's own assertions (which fire on .file/.path) don't.
const expr = `age: let
  ph = builtins.hashString "sha256" age.rekey.hostPubkey;
in builtins.mapAttrs (_: s: let
  rf = s.rekeyFile or null;
  has = rf != null && builtins.pathExists rf;
  id = builtins.substring 0 32 (builtins.hashString "sha256" (ph + builtins.hashFile "sha256" rf));
in {
  rekeyFile = if rf == null then null else toString rf;
  masterTracked = has;
  generator = (s.generator or null) != null;
  owner = s.owner or null;
  intermediary = s.intermediary or false;
  expected = if has then "${id}-${s.name}.age" else null;
}) age.secrets`

type nixSecret struct {
	RekeyFile     *string `json:"rekeyFile"`
	MasterTracked bool    `json:"masterTracked"`
	Generator     bool    `json:"generator"`
	Owner         *string `json:"owner"`
	Expected      *string `json:"expected"`
	Intermediary  bool    `json:"intermediary"`
}

// Secret is one declared secret's state.
type Secret struct {
	Name      string `json:"name"`
	RekeyFile string `json:"rekey_file,omitempty"` // repo-relative
	Generator bool   `json:"generator"`
	Owner     string `json:"owner,omitempty"`
	Rekeyed   string `json:"rekeyed,omitempty"` // repo-relative expected path
	// ok | intermediary | missing_master | untracked_master | stale_or_missing_rekey | untracked_rekey | no_rekey_file
	State string `json:"state"`
}

// Fix is one thing to do, and who does it.
type Fix struct {
	Actor         string `json:"actor"` // agent | human
	Command       string `json:"command"`
	Why           string `json:"why"`
	NeedsKeyTouch bool   `json:"needs_key_touch,omitempty"`
}

// Report is the result for one host.
type Report struct {
	Host    string   `json:"host"`
	OK      int      `json:"ok"`
	Secrets []Secret `json:"secrets"`
	Orphans []string `json:"orphans,omitempty"` // rekeyed files nothing expects
	Fixes   []Fix    `json:"fixes,omitempty"`
}

// Check evaluates one host. kind is nixosConfigurations or systemConfigs.
func Check(ctx context.Context, root, host, kind string) (*Report, error) {
	ctx, cancel := context.WithTimeout(ctx, 5*time.Minute)
	defer cancel()
	attr := fmt.Sprintf(".#%s.%s.config.age", kind, host)
	cmd := exec.CommandContext(ctx, "nix", "eval", "--json", attr, "--apply", expr)
	cmd.Dir = root
	out, err := cmd.Output()
	if err != nil {
		msg := err.Error()
		if ee, ok := err.(*exec.ExitError); ok {
			msg = lastLines(string(ee.Stderr), 15)
		}
		return nil, fmt.Errorf("evaluating %s: %s", attr, msg)
	}
	var decl map[string]nixSecret
	if err := json.Unmarshal(out, &decl); err != nil {
		return nil, err
	}

	r := &Report{Host: host}
	storageRel := filepath.Join("secrets", "hosts", host)
	expected := map[string]bool{}
	tracked := trackedFiles(ctx, root)

	names := make([]string, 0, len(decl))
	for n := range decl {
		names = append(names, n)
	}
	sort.Strings(names)
	needRekey, needAdd, needGenerate := false, []string{}, false
	for _, n := range names {
		d := decl[n]
		s := Secret{Name: n, Generator: d.Generator}
		if d.Owner != nil {
			s.Owner = *d.Owner
		}
		if d.RekeyFile == nil {
			s.State = "no_rekey_file"
			r.Secrets = append(r.Secrets, s)
			continue
		}
		s.RekeyFile = repoRel(*d.RekeyFile)
		switch {
		case d.Intermediary && d.MasterTracked:
			// Only an input to other secrets' generators: never rekeyed
			// for a host.
			s.State = "intermediary"
			r.OK++
		case d.MasterTracked && d.Expected != nil:
			s.Rekeyed = filepath.Join(storageRel, *d.Expected)
			expected[filepath.Base(s.Rekeyed)] = true
			switch {
			case tracked[s.Rekeyed]:
				s.State = "ok"
				r.OK++
			case fileExists(filepath.Join(root, s.Rekeyed)):
				s.State = "untracked_rekey"
				needAdd = append(needAdd, s.Rekeyed)
			default:
				s.State = "stale_or_missing_rekey"
				needRekey = true
			}
		case fileExists(filepath.Join(root, s.RekeyFile)):
			s.State = "untracked_master"
			needAdd = append(needAdd, s.RekeyFile)
			needRekey = true
		default:
			s.State = "missing_master"
			if d.Generator {
				needGenerate = true
			}
			needRekey = true
		}
		r.Secrets = append(r.Secrets, s)
	}

	if entries, err := os.ReadDir(filepath.Join(root, storageRel)); err == nil {
		for _, e := range entries {
			if n := e.Name(); strings.HasSuffix(n, ".age") && !expected[n] {
				r.Orphans = append(r.Orphans, filepath.Join(storageRel, n))
			}
		}
	}

	if needGenerate {
		r.Fixes = append(r.Fixes, Fix{Actor: "agent", Command: "just generate-secrets", Why: "create the master files of newly declared generated secrets (no key touch)"})
	}
	missingManual := false
	for _, s := range r.Secrets {
		if s.State == "missing_master" && !s.Generator {
			missingManual = true
			r.Fixes = append(r.Fixes, Fix{Actor: "human", Command: "agenix edit " + s.RekeyFile, Why: s.Name + " has no master file and no generator: the human enters it", NeedsKeyTouch: true})
		}
	}
	if len(needAdd) > 0 {
		r.Fixes = append(r.Fixes, Fix{Actor: "agent", Command: "git add " + strings.Join(needAdd, " "), Why: "flakes only see tracked files"})
	}
	if needRekey || missingManual {
		r.Fixes = append(r.Fixes, Fix{Actor: "human", Command: "just rekey", Why: "encrypt the master files for each host into secrets/hosts/", NeedsKeyTouch: true})
		r.Fixes = append(r.Fixes, Fix{Actor: "agent", Command: "git add secrets/hosts", Why: "after the rekey: track the new rekeyed files, then commit with a pathspec"})
	}
	if len(r.Orphans) > 0 {
		r.Fixes = append(r.Fixes, Fix{Actor: "agent", Command: "git rm " + strings.Join(r.Orphans, " "), Why: "rekeyed files no declared secret expects (left over from an old master file or a removed secret); only once the host evaluates cleanly"})
	}
	return r, nil
}

// repoRel maps a flake-source store path back to the checkout.
func repoRel(p string) string {
	if i := strings.Index(p, "-source/"); i >= 0 && strings.HasPrefix(p, "/nix/store/") {
		return p[i+len("-source/"):]
	}
	return p
}

func trackedFiles(ctx context.Context, root string) map[string]bool {
	out, err := exec.CommandContext(ctx, "git", "-C", root, "ls-files", "-z").Output()
	m := map[string]bool{}
	if err != nil {
		return m
	}
	for _, f := range strings.Split(string(out), "\x00") {
		if f != "" {
			m[f] = true
		}
	}
	return m
}

func fileExists(p string) bool {
	_, err := os.Stat(p)
	return err == nil
}

func lastLines(s string, n int) string {
	l := strings.Split(strings.TrimRight(s, "\n"), "\n")
	if len(l) > n {
		l = l[len(l)-n:]
	}
	return strings.Join(l, "\n")
}
