package main

import (
	"strings"
	"testing"
)

func TestTemplateByName(t *testing.T) {
	for _, want := range builtinTemplates {
		t.Run(want.Name, func(t *testing.T) {
			got := templateByName(want.Name)
			if got == nil {
				t.Fatalf("templateByName(%q) = nil, want the %s template", want.Name, want.Name)
			}
			if got.Name != want.Name {
				t.Errorf("got template %q, want %q", got.Name, want.Name)
			}
		})
	}

	t.Run("unknown", func(t *testing.T) {
		if got := templateByName("haskell"); got != nil {
			t.Errorf("templateByName(\"haskell\") = %+v, want nil", got)
		}
	})

	t.Run("empty", func(t *testing.T) {
		if got := templateByName(""); got != nil {
			t.Errorf("templateByName(\"\") = %+v, want nil", got)
		}
	})
}

// TestEmbeddedTemplateFiles guards the //go:embed contract: every built-in
// template must have a devenv.nix on disk, or injection fails at runtime for
// an environment that already reported "creating".
func TestEmbeddedTemplateFiles(t *testing.T) {
	for _, tmpl := range builtinTemplates {
		t.Run(tmpl.Name, func(t *testing.T) {
			content, err := templateFS.ReadFile("templates/" + tmpl.Name + "/devenv.nix")
			if err != nil {
				t.Fatalf("missing embedded devenv.nix: %v", err)
			}
			if len(content) == 0 {
				t.Error("devenv.nix is empty")
			}
			if !strings.Contains(string(content), "{") {
				t.Errorf("devenv.nix does not look like a Nix attrset:\n%s", content)
			}
		})
	}

	t.Run("shared files", func(t *testing.T) {
		for _, name := range []string{"templates/flake.nix", "templates/envrc"} {
			content, err := templateFS.ReadFile(name)
			if err != nil {
				t.Fatalf("missing embedded %s: %v", name, err)
			}
			if len(content) == 0 {
				t.Errorf("%s is empty", name)
			}
		}
	})
}

func TestBuiltinTemplateIndicatorsAreUnique(t *testing.T) {
	seen := map[string]string{}
	for _, tmpl := range builtinTemplates {
		if len(tmpl.Indicators) == 0 {
			t.Errorf("template %q has no indicators, so auto-detection can never select it", tmpl.Name)
		}
		if tmpl.Description == "" {
			t.Errorf("template %q has no description", tmpl.Name)
		}
		for _, ind := range tmpl.Indicators {
			if prev, dup := seen[ind]; dup {
				t.Errorf("indicator %q is claimed by both %q and %q; detection order silently decides the winner", ind, prev, tmpl.Name)
			}
			seen[ind] = tmpl.Name
		}
	}
}

func TestBuiltinTemplateNamesAreUnique(t *testing.T) {
	seen := map[string]bool{}
	for _, tmpl := range builtinTemplates {
		if seen[tmpl.Name] {
			t.Errorf("duplicate template name %q", tmpl.Name)
		}
		seen[tmpl.Name] = true
	}
}

// TestWellKnownReposAreValid keeps the placeholder map honest: entries must
// have both a name and a URL, since a blank URL would make env_create clone
// nothing and silently succeed.
func TestWellKnownReposAreValid(t *testing.T) {
	for name, url := range wellKnownRepos {
		if name == "" {
			t.Error("well-known repo with an empty name")
		}
		if url == "" {
			t.Errorf("well-known repo %q has an empty URL", name)
		}
		if templateByName(name) != nil {
			t.Errorf("well-known repo %q shadows a built-in template name", name)
		}
	}
}
