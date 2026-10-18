package watch

import (
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

func TestRepoFilter(t *testing.T) {
	_, file, _, _ := runtime.Caller(0)
	root := filepath.Join(filepath.Dir(file), "..", "..", "..", "..")
	if _, err := os.Stat(filepath.Join(root, ".forgejo", "workflows", "check.yml")); err != nil {
		t.Skip("not in the nix-config checkout")
	}
	f, err := Load(root, "check.yml")
	if err != nil {
		t.Fatal(err)
	}
	cases := map[string]bool{
		"flake.nix":                                true,
		"nixos/romeo/default.nix":                  true,
		"secrets/hosts/romeo-2/abc-x.age":          true,
		"nixos/whiskey/services/blueprints/a.yaml": true,
		"flake.lock":                               true,
		".forgejo/scripts/check-hosts.sh":          true,
		".forgejo/workflows/check.yml":             true,
		".forgejo/workflows/update.yml":            false,
		"pkgs/nixcfg-mcp/main.go":                  false,
		"AGENTS.md":                                false,
		"justfile":                                 false,
		"home-manager/x/y.py":                      false,
	}
	for p, want := range cases {
		w, _ := f.Classify([]string{p})
		if got := len(w) == 1; got != want {
			t.Errorf("%s: watched=%v want %v", p, got, want)
		}
	}
}

func TestGlob(t *testing.T) {
	cases := []struct {
		pat, path string
		want      bool
	}{
		{"**.nix", "a.nix", true},
		{"**.nix", "a/b/c.nix", true},
		{"*.nix", "a/b.nix", false},
		{"docs/**", "docs/a/b.md", true},
		{"**/x.md", "x.md", true},
		{"nixos/**/*.yml", "nixos/a/b/c.yml", true},
		{"nixos/**/*.yml", "nixos/c.yml", true},
		{"a/{b,c}.txt", "a/c.txt", true},
	}
	for _, c := range cases {
		if got := globRegexp(c.pat).MatchString(c.path); got != c.want {
			t.Errorf("%s ~ %s = %v want %v", c.pat, c.path, got, c.want)
		}
	}
}
