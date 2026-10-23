package guard

import (
	"bytes"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

func TestCommands(t *testing.T) {
	cases := []struct {
		cmd  string
		rule string // "" means allowed
	}{
		// commit-pathspec
		{`git commit -m x -- a b`, ""},
		{`git commit -m "fix(x): y" -- nixos/romeo/default.nix`, ""},
		{`git commit -m x nixos/a.nix`, ""},
		{`git -C /tmp/r commit -m x -- f`, ""},
		{`git commit --dry-run`, ""},
		{`git commit --pathspec-from-file=list`, ""},
		{`git commit -m x`, "commit-pathspec"},
		{`git commit -m "$(cat msg)"`, "commit-pathspec"},
		{`git commit -am x`, "commit-pathspec"},
		{`git commit -a -m x -- f`, "commit-pathspec"},
		{`git commit --amend --no-edit`, "commit-pathspec"},
		{`git commit -p`, "commit-pathspec"},
		{`git commit -F msg.txt`, "commit-pathspec"},
		{`git commit -m x --`, "commit-pathspec"},
		{`cd /x && git commit -m x`, "commit-pathspec"},
		{`bash -c 'git commit -am x'`, "commit-pathspec"},
		{`bash -lc "git commit -m x"`, "commit-pathspec"},
		{`sudo -u bcnelson git commit -m x`, "commit-pathspec"},
		{`env FOO=1 git commit -m x`, "commit-pathspec"},
		{`FOO=1 git commit -m x`, "commit-pathspec"},
		{`nix develop -c git commit -m x`, "commit-pathspec"},
		{`timeout 10 git commit -m x`, "commit-pathspec"},
		{`echo "git commit -m x"`, ""},
		{`git log --oneline -5`, ""},

		// pushing main is fine; force pushes and writing auto-update are not
		{`git push origin main`, ""},
		{`git push`, ""},
		{`git push -u origin feature`, ""},
		{`git push --force origin main`, "push-force"},
		{`git push -f`, "push-force"},
		{`git push --force-with-lease origin main`, "push-force"},
		{`git push origin +main`, "push-force"},
		{`git push origin --delete old`, "push-force"},
		{`git push origin main:auto-update`, "deploy-control"},
		{`git push origin HEAD:refs/heads/auto-update`, "deploy-control"},
		{`git push origin auto-update`, "deploy-control"},
		{`git fetch origin`, ""},

		// secret-read
		{`cat nixos/sensitive.nix`, "secret-read"},
		{`head -5 nixos/romeo/sensitive.nix`, "secret-read"},
		{`less < nixos/sensitive.nix`, "secret-read"},
		{`nix eval --file nixos/sensitive.nix gh_token`, "secret-read"},
		{`nix eval --expr '(import ./nixos/sensitive.nix).x'`, "secret-read"},
		{`git show HEAD:nixos/sensitive.nix`, "secret-read"},
		{`cp nixos/sensitive.nix /tmp/x`, "secret-read"},
		{`ssh romeo cat /run/agenix/ntfy_topic`, "secret-read"},
		{`cat /run/agenix/foo`, "secret-read"},
		{`age -d -i key secret.age`, "secret-read"},
		{`agenix edit secrets/store/x.age`, "secret-read"},
		{`agenix view secrets/store/x.age`, "secret-read"},
		{`env FOO=1 sudo agenix edit s.age`, "secret-read"},
		{`gpg --decrypt local.key.asc`, "secret-read"},
		{`bw get password x`, "secret-read"},
		{`ls -l nixos/sensitive.nix`, ""},
		{`git add nixos/sensitive.nix`, ""},
		{`git status --short`, ""},
		{`git log --oneline -- nixos/sensitive.nix`, ""},
		{`git log -p -- nixos/sensitive.nix`, "secret-read"},
		{`git diff --stat`, ""},
		{`grep -n sensitive.nix lib/default.nix`, ""},
		{`rg getSecret nixos`, ""},
		{`ls /run/agenix`, ""},

		// key-touch
		{`just rekey`, "key-touch"},
		{`just new-secrets`, "key-touch"},
		{`just sync-secrets`, "key-touch"},
		{`just unlock`, "key-touch"},
		{`just tofu plan`, "key-touch"},
		{`just push`, "key-touch"},
		{`agenix rekey -a`, "key-touch"},
		{`git-crypt unlock`, "key-touch"},
		{`git crypt unlock`, "key-touch"},
		{`git-crypt status`, ""},
		{`agenix generate -a`, ""},
		{`just generate-secrets`, ""},
		{`just check-host romeo-2`, ""},
		{`just fmt`, ""},
		{`echo "just rekey"`, ""},
		{`nix run .#age-bitwarden-sync -- --fido`, "key-touch"},
		{`tofu apply`, "human-only"},
		{`tofu validate`, ""},

		// deploy-control
		{`curl -d hi https://ntfy.sh/topic`, "deploy-control"},
		{`systemctl start auto-update.service`, "deploy-control"},
		{`sudo systemctl --no-block start auto-update`, "deploy-control"},
		{`ssh root@romeo systemctl restart auto-update.service`, "deploy-control"},
		{`systemctl stop auto-update`, "deploy-control"},
		{`systemctl status auto-update`, ""},
		{`systemctl show auto-update.service -p Result`, ""},
		{`journalctl -u auto-update`, ""},
		{`echo BUILT_COMMIT= > /var/lib/auto-update/state`, "deploy-control"},
		{`sudo sed -i s/x/y/ /var/lib/auto-update/state`, "deploy-control"},
		{`sudo rm /var/lib/auto-update/state`, "deploy-control"},
		{`cat /var/lib/auto-update/state`, ""},
		{`curl -X POST https://git.bcnelson.dev/api/v1/repos/x/y/actions/workflows/check.yml/dispatches`, "deploy-control"},
		{`curl -fsS https://git.bcnelson.dev/api/v1/repos/bcnelson/nix-config/commits/abc/status`, ""},

		// activation
		{`sudo nixos-rebuild switch --flake .#romeo-2`, "activation"},
		{`nixos-rebuild boot --flake .`, "activation"},
		{`nixos-rebuild build --flake .#romeo-2`, ""},
		{`nixos-rebuild dry-build --flake .#romeo-2`, ""},
		{`just update-os`, "activation"},
		{`just apply`, "activation"},
		{`just o`, "activation"},
		{`just home`, "activation"},
		{`home-manager switch --flake .#bcnelson@sierra`, "activation"},
		{`home-manager build --flake .#bcnelson@sierra`, ""},
		{`/nix/store/abc-nixos-system/bin/switch-to-configuration switch`, "activation"},
		{`sudo reboot`, "activation"},
		{`ssh root@romeo systemctl reboot`, "activation"},
		{`nix build .#nixosConfigurations.romeo-2.config.system.build.toplevel`, ""},

		// odds and ends
		{`ls -la && git status`, ""},
		{`for f in *.nix; do echo $f; done`, ""},
		{`(cd x && just rekey)`, "key-touch"},
		{`echo $(just rekey)`, "key-touch"},
		{`ssh -o BatchMode=yes bcnelson@romeo.b.nel.family 'git -C /config log -1'`, ""},
	}
	for _, tc := range cases {
		d := CheckCommand(tc.cmd)
		if d.Rule != tc.rule || d.Block != (tc.rule != "") {
			t.Errorf("%q: got block=%v rule=%q, want rule=%q (%s)", tc.cmd, d.Block, d.Rule, tc.rule, d.Reason)
		}
	}
}

// Recursive searches only trip when they would actually reach a sensitive file.
func TestRecursiveSearch(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("needs git (not in the nix build sandbox)")
	}
	dir := t.TempDir()
	run := func(args ...string) {
		t.Helper()
		if err := gitRun(dir, args...); err != nil {
			t.Fatal(err)
		}
	}
	run("init", "-q")
	must(t, os.MkdirAll(filepath.Join(dir, "nixos"), 0o755))
	must(t, os.MkdirAll(filepath.Join(dir, "pkgs"), 0o755))
	must(t, os.WriteFile(filepath.Join(dir, "nixos", "sensitive.nix"), []byte("{}"), 0o644))
	must(t, os.WriteFile(filepath.Join(dir, "pkgs", "a.nix"), []byte("{}"), 0o644))
	run("add", ".")

	old, _ := os.Getwd()
	must(t, os.Chdir(dir))
	defer os.Chdir(old)
	sensitiveOnce = sync.Once{}
	sensitiveFiles = nil

	cases := []struct {
		cmd   string
		block bool
	}{
		{`grep -rn token .`, true},
		{`grep -rn token nixos`, true},
		{`grep -rn token pkgs`, false},
		{`grep -rn --exclude=sensitive.nix token .`, false},
		{`grep -n token pkgs/a.nix`, false},
		{`rg token`, false},
		{`rg -uu token`, true},
		{`rg --no-ignore token nixos`, true},
		{`git grep token`, true},
		{`git grep token -- ':!**/sensitive.nix'`, false},
		{`cat nixos/*.nix`, true},
		{`cat pkgs/*.nix`, false},
		// cd is followed: the hook runs from the root, the command doesn't.
		{`cd pkgs && grep -rn token .`, false},
		{`cd nixos && grep -rn token .`, true},
		{`cd pkgs && cat ../nixos/*.nix`, true},
	}
	for _, tc := range cases {
		d := CheckCommand(tc.cmd)
		if d.Block != tc.block {
			t.Errorf("%q: got block=%v (%s), want %v", tc.cmd, d.Block, d.Reason, tc.block)
		}
	}
}

func TestPayloads(t *testing.T) {
	cases := []struct {
		agent   string
		payload string
		block   bool
	}{
		{"claude", `{"tool_name":"Bash","tool_input":{"command":"git commit -m x"}}`, true},
		{"claude", `{"tool_name":"Bash","tool_input":{"command":"git commit -m x -- a"}}`, false},
		{"claude", `{"tool_name":"Read","tool_input":{"file_path":"/home/u/nix-config/nixos/sensitive.nix"}}`, true},
		{"claude", `{"tool_name":"Read","tool_input":{"file_path":"/home/u/nix-config/flake.nix"}}`, false},
		{"claude", `{"tool_name":"Grep","tool_input":{"pattern":"x","path":"nixos/sensitive.nix"}}`, true},
		{"claude", `{"tool_name":"Glob","tool_input":{"pattern":"**/sensitive.nix"}}`, false},
		{"claude", `{"tool_name":"mcp__ssh__ssh_exec","tool_input":{"host":"romeo","command":"cat /run/agenix/x"}}`, true},
		{"claude", `{"tool_name":"mcp__ssh__ssh_exec","tool_input":{"host":"romeo","command":"systemctl status auto-update"}}`, false},
		{"codex", `{"tool_name":"Bash","tool_input":{"command":["bash","-lc","just rekey"]}}`, true},
		{"codex", `{"tool_name":"Bash","tool_input":{"command":["git","status"]}}`, false},
		{"opencode", `{"tool_name":"read","tool_input":{"filePath":"nixos/whiskey/sensitive.nix"}}`, true},
		{"pi", `{"tool_name":"bash","tool_input":{"command":"just update-os"}}`, true},
		{"plain", `git push origin main`, false},
		{"plain", `git push -f origin main`, true},
	}
	for _, tc := range cases {
		var out, errb bytes.Buffer
		code := RunHook(tc.agent, strings.NewReader(tc.payload), &out, &errb)
		var blocked bool
		switch tc.agent {
		case "claude", "codex":
			if code != 0 {
				t.Errorf("%s %s: exit %d", tc.agent, tc.payload, code)
			}
			var v struct {
				H struct {
					D string `json:"permissionDecision"`
				} `json:"hookSpecificOutput"`
			}
			if out.Len() > 0 {
				must(t, json.Unmarshal(out.Bytes(), &v))
			}
			blocked = v.H.D == "deny"
		default:
			var v Decision
			must(t, json.Unmarshal(out.Bytes(), &v))
			blocked = v.Block
			if blocked != (code == 2) {
				t.Errorf("%s %s: exit %d disagrees with block=%v", tc.agent, tc.payload, code, blocked)
			}
		}
		if blocked != tc.block {
			t.Errorf("%s %s: blocked=%v want %v (%s)", tc.agent, tc.payload, blocked, tc.block, out.String())
		}
	}
}

func TestMessageHasHumanCommand(t *testing.T) {
	d := CheckCommand("just rekey")
	if !strings.Contains(d.Message(), "Hand this to the human, on one line: just rekey") {
		t.Fatalf("message lacks the hand-off: %s", d.Message())
	}
}

func must(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

func gitRun(dir string, args ...string) error {
	cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
	cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@t", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@t")
	return cmd.Run()
}
