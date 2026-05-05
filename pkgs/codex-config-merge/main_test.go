package main

import "testing"

func TestExtractRuntimeState(t *testing.T) {
	doc := configMap{
		"model": "gpt-5.4",
		"projects": configMap{
			"/tmp/trusted": configMap{
				"trust_level": "trusted",
				"ignored":     true,
			},
			"/tmp/untrusted": configMap{
				"trust_level": "untrusted",
			},
			"/tmp/missing": configMap{
				"other": "value",
			},
		},
	}

	got := extractRuntimeState(doc)
	projects := getProjectsTable(got)

	if len(projects) != 2 {
		t.Fatalf("expected 2 runtime projects, got %d", len(projects))
	}

	assertTrustLevel(t, projects, "/tmp/trusted", "trusted")
	assertTrustLevel(t, projects, "/tmp/untrusted", "untrusted")

	if _, exists := projects["/tmp/missing"]; exists {
		t.Fatalf("expected project without trust_level to be omitted")
	}
}

func TestMergeConfigPreservesRuntimeTrustLevel(t *testing.T) {
	base := configMap{
		"experimental_use_rmcp_client": true,
	}
	runtime := configMap{
		"projects": configMap{
			"/tmp/repo": configMap{
				"trust_level": "trusted",
			},
		},
	}

	merged := mergeConfig(base, runtime)
	projects := getProjectsTable(merged)
	assertTrustLevel(t, projects, "/tmp/repo", "trusted")
}

func TestMergeConfigPrefersBaseTrustLevel(t *testing.T) {
	base := configMap{
		"projects": configMap{
			"/tmp/repo": configMap{
				"trust_level": "untrusted",
			},
		},
	}
	runtime := configMap{
		"projects": configMap{
			"/tmp/repo": configMap{
				"trust_level": "trusted",
			},
		},
	}

	merged := mergeConfig(base, runtime)
	projects := getProjectsTable(merged)
	assertTrustLevel(t, projects, "/tmp/repo", "untrusted")
}

func TestMergeConfigKeepsProjectMetadata(t *testing.T) {
	base := configMap{
		"projects": configMap{
			"/tmp/repo": configMap{
				"note": "keep-me",
			},
		},
	}
	runtime := configMap{
		"projects": configMap{
			"/tmp/repo": configMap{
				"trust_level": "trusted",
			},
		},
	}

	merged := mergeConfig(base, runtime)
	projects := getProjectsTable(merged)
	project, ok := asConfigMap(projects["/tmp/repo"])
	if !ok {
		t.Fatalf("expected merged project table")
	}

	if got := project["note"]; got != "keep-me" {
		t.Fatalf("expected note to survive merge, got %#v", got)
	}

	if got := project["trust_level"]; got != "trusted" {
		t.Fatalf("expected runtime trust_level to be merged, got %#v", got)
	}
}

func assertTrustLevel(t *testing.T, projects configMap, projectPath string, want string) {
	t.Helper()

	project, ok := asConfigMap(projects[projectPath])
	if !ok {
		t.Fatalf("expected project %q to be present", projectPath)
	}

	got, ok := project["trust_level"].(string)
	if !ok {
		t.Fatalf("expected project %q to have a string trust_level", projectPath)
	}

	if got != want {
		t.Fatalf("expected project %q trust_level %q, got %q", projectPath, want, got)
	}
}
