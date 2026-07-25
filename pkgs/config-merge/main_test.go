package main

import (
	"os"
	"path/filepath"
	"testing"
)

// codexKeys is the pattern set that reproduces the original, codex-specific
// behaviour this daemon was generalised from.
var codexKeys = mustPatterns("projects.*.trust_level")

// herdrKeys covers the settings herdr's TUI writes back to its own config.
var herdrKeys = mustPatterns("ui.sound.enabled", "ui.agent_panel_sort", "experimental.pane_history")

func mustPatterns(keys ...string) []keyPattern {
	patterns, err := parsePatterns(keys)
	if err != nil {
		panic(err)
	}

	return patterns
}

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

	got := extractRuntimeState(doc, codexKeys)
	projects, ok := asConfigMap(got["projects"])
	if !ok {
		t.Fatalf("expected a projects table")
	}

	if len(projects) != 2 {
		t.Fatalf("expected 2 runtime projects, got %d", len(projects))
	}

	assertTrustLevel(t, projects, "/tmp/trusted", "trusted")
	assertTrustLevel(t, projects, "/tmp/untrusted", "untrusted")

	if _, exists := projects["/tmp/missing"]; exists {
		t.Fatalf("expected project without trust_level to be omitted")
	}

	// Keys outside the pattern set must not leak into the overlay.
	if _, exists := got["model"]; exists {
		t.Fatalf("expected unmatched top-level keys to be dropped")
	}

	trusted, _ := asConfigMap(projects["/tmp/trusted"])
	if _, exists := trusted["ignored"]; exists {
		t.Fatalf("expected sibling keys of a matched path to be dropped")
	}
}

func TestExtractRuntimeStateNestedKeys(t *testing.T) {
	doc := configMap{
		"onboarding": false,
		"theme":      configMap{"name": "catppuccin"},
		"ui": configMap{
			"sound":            configMap{"enabled": false, "path": "beep.mp3"},
			"agent_panel_sort": "priority",
		},
		"experimental": configMap{"pane_history": true},
	}

	got := extractRuntimeState(doc, herdrKeys)

	if value, _ := getPath(got, []string{"ui", "sound", "enabled"}); value != false {
		t.Fatalf("expected ui.sound.enabled to be extracted, got %#v", value)
	}

	if value, _ := getPath(got, []string{"ui", "agent_panel_sort"}); value != "priority" {
		t.Fatalf("expected ui.agent_panel_sort to be extracted, got %#v", value)
	}

	if value, _ := getPath(got, []string{"experimental", "pane_history"}); value != true {
		t.Fatalf("expected experimental.pane_history to be extracted, got %#v", value)
	}

	if _, exists := getPath(got, []string{"ui", "sound", "path"}); exists {
		t.Fatalf("expected unmatched ui.sound.path to be dropped")
	}

	if _, exists := got["theme"]; exists {
		t.Fatalf("expected unmatched theme table to be dropped")
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

	merged := mergeConfig(base, runtime, codexKeys)
	projects, _ := asConfigMap(merged["projects"])
	assertTrustLevel(t, projects, "/tmp/repo", "trusted")

	if merged["experimental_use_rmcp_client"] != true {
		t.Fatalf("expected base keys to survive the merge")
	}
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

	merged := mergeConfig(base, runtime, codexKeys)
	projects, _ := asConfigMap(merged["projects"])
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

	merged := mergeConfig(base, runtime, codexKeys)
	projects, _ := asConfigMap(merged["projects"])
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

func TestMergeConfigNestedBaseWins(t *testing.T) {
	base := configMap{
		"ui": configMap{
			"agent_panel_sort": "spaces",
			"sidebar_width":    32,
		},
	}
	runtime := configMap{
		"ui": configMap{
			"agent_panel_sort": "priority",
			"sound":            configMap{"enabled": false},
		},
		"experimental": configMap{"pane_history": true},
	}

	merged := mergeConfig(base, runtime, herdrKeys)

	if value, _ := getPath(merged, []string{"ui", "agent_panel_sort"}); value != "spaces" {
		t.Fatalf("expected base to win for ui.agent_panel_sort, got %#v", value)
	}

	if value, _ := getPath(merged, []string{"ui", "sound", "enabled"}); value != false {
		t.Fatalf("expected runtime ui.sound.enabled to apply, got %#v", value)
	}

	if value, _ := getPath(merged, []string{"ui", "sidebar_width"}); value != 32 {
		t.Fatalf("expected unrelated base keys to survive, got %#v", value)
	}

	if value, _ := getPath(merged, []string{"experimental", "pane_history"}); value != true {
		t.Fatalf("expected runtime to create missing tables, got %#v", value)
	}
}

func TestMergeConfigIgnoresUnmatchedRuntimeKeys(t *testing.T) {
	base := configMap{"theme": configMap{"name": "catppuccin"}}
	runtime := configMap{
		"theme":  configMap{"name": "gruvbox"},
		"secret": "should-not-apply",
	}

	merged := mergeConfig(base, runtime, herdrKeys)

	if value, _ := getPath(merged, []string{"theme", "name"}); value != "catppuccin" {
		t.Fatalf("expected base theme to win, got %#v", value)
	}

	if _, exists := merged["secret"]; exists {
		t.Fatalf("expected runtime keys outside the pattern set to be ignored")
	}
}

func TestSubtractDeclaredKeepsUndeclaredKeys(t *testing.T) {
	base := configMap{
		"theme": configMap{"name": "catppuccin"},
		"keys":  configMap{"prefix": "ctrl+a"},
	}
	live := configMap{
		// Declared by base: nothing to carry over.
		"theme": configMap{"name": "gruvbox"},
		"keys":  configMap{"prefix": "ctrl+a"},
		// A new field inside a table the base also touches.
		"ui": configMap{"sidebar_width": 40},
		// A table the base never mentions.
		"experimental": configMap{"pane_history": true},
	}

	remainder := subtractDeclared(live, base)

	if _, exists := remainder["theme"]; exists {
		t.Fatalf("expected declared theme table to be dropped, got %#v", remainder["theme"])
	}

	if _, exists := remainder["keys"]; exists {
		t.Fatalf("expected declared keys table to be dropped")
	}

	if value, _ := getPath(remainder, []string{"ui", "sidebar_width"}); value != 40 {
		t.Fatalf("expected undeclared ui.sidebar_width to be kept, got %#v", value)
	}

	if value, _ := getPath(remainder, []string{"experimental", "pane_history"}); value != true {
		t.Fatalf("expected undeclared experimental table to be kept, got %#v", value)
	}
}

func TestSubtractDeclaredDescendsSharedTables(t *testing.T) {
	base := configMap{"ui": configMap{"sound": configMap{"path": "beep.mp3"}}}
	live := configMap{
		"ui": configMap{
			"sound": configMap{"path": "beep.mp3", "enabled": false},
		},
	}

	remainder := subtractDeclared(live, base)

	if value, _ := getPath(remainder, []string{"ui", "sound", "enabled"}); value != false {
		t.Fatalf("expected the undeclared sibling to be kept, got %#v", value)
	}

	if _, exists := getPath(remainder, []string{"ui", "sound", "path"}); exists {
		t.Fatalf("expected the declared sibling to be dropped")
	}
}

func TestOverlayUndeclaredPrefersBase(t *testing.T) {
	base := configMap{
		"theme": configMap{"name": "catppuccin"},
		"ui":    configMap{"sidebar_width": 32},
	}
	runtime := configMap{
		"theme": configMap{"name": "gruvbox"},
		"ui":    configMap{"agent_panel_sort": "priority"},
		"experimental": configMap{
			"pane_history": true,
		},
	}

	merged := overlayUndeclared(base, runtime)

	if value, _ := getPath(merged, []string{"theme", "name"}); value != "catppuccin" {
		t.Fatalf("expected base to win for theme.name, got %#v", value)
	}

	if value, _ := getPath(merged, []string{"ui", "sidebar_width"}); value != 32 {
		t.Fatalf("expected base ui.sidebar_width to survive, got %#v", value)
	}

	if value, _ := getPath(merged, []string{"ui", "agent_panel_sort"}); value != "priority" {
		t.Fatalf("expected undeclared runtime key to apply, got %#v", value)
	}

	if value, _ := getPath(merged, []string{"experimental", "pane_history"}); value != true {
		t.Fatalf("expected undeclared runtime table to apply, got %#v", value)
	}
}

// The round trip a preserve-unknown instance actually performs: harvest from
// live, then replay onto base.
func TestPreserveUnknownRoundTrip(t *testing.T) {
	d := &daemon{preserveUnknown: true}

	base := configMap{"theme": configMap{"name": "catppuccin"}}
	live := configMap{
		"theme": configMap{"name": "gruvbox"},
		"ui":    configMap{"sidebar_width": 40},
	}

	merged := d.merge(base, d.extract(live, base))

	if value, _ := getPath(merged, []string{"theme", "name"}); value != "catppuccin" {
		t.Fatalf("expected the declared theme to be restored, got %#v", value)
	}

	if value, _ := getPath(merged, []string{"ui", "sidebar_width"}); value != 40 {
		t.Fatalf("expected the undeclared key to survive, got %#v", value)
	}
}

// The same input under the default mode: the undeclared key is discarded.
func TestStrictModeDropsUndeclaredKeys(t *testing.T) {
	d := &daemon{patterns: mustPatterns("ui.agent_panel_sort")}

	base := configMap{"theme": configMap{"name": "catppuccin"}}
	live := configMap{
		"theme": configMap{"name": "gruvbox"},
		"ui":    configMap{"sidebar_width": 40, "agent_panel_sort": "priority"},
	}

	merged := d.merge(base, d.extract(live, base))

	if _, exists := getPath(merged, []string{"ui", "sidebar_width"}); exists {
		t.Fatalf("expected the unlisted key to be dropped in strict mode")
	}

	if value, _ := getPath(merged, []string{"ui", "agent_panel_sort"}); value != "priority" {
		t.Fatalf("expected the listed key to survive, got %#v", value)
	}
}

func TestRoundTripJSON(t *testing.T) {
	doc := configMap{
		"hooks": configMap{
			"SessionStart": []any{configMap{"matcher": "*"}},
		},
		"theme": "dark",
	}

	encoded, err := encodeConfig("json", doc)
	if err != nil {
		t.Fatalf("encode json: %v", err)
	}

	decoded, err := decodeConfig("json", []byte(encoded))
	if err != nil {
		t.Fatalf("decode json: %v", err)
	}

	if decoded["theme"] != "dark" {
		t.Fatalf("expected theme to survive the round trip, got %#v", decoded["theme"])
	}

	if _, ok := getPath(decoded, []string{"hooks", "SessionStart"}); !ok {
		t.Fatalf("expected nested hooks to survive the round trip")
	}
}

func TestRoundTripTOML(t *testing.T) {
	doc := configMap{
		"onboarding": false,
		"ui":         configMap{"sound": configMap{"enabled": true}},
	}

	encoded, err := encodeConfig("toml", doc)
	if err != nil {
		t.Fatalf("encode toml: %v", err)
	}

	decoded, err := decodeConfig("toml", []byte(encoded))
	if err != nil {
		t.Fatalf("decode toml: %v", err)
	}

	if decoded["onboarding"] != false {
		t.Fatalf("expected onboarding to survive the round trip, got %#v", decoded["onboarding"])
	}

	if value, _ := getPath(decoded, []string{"ui", "sound", "enabled"}); value != true {
		t.Fatalf("expected nested table to survive the round trip, got %#v", value)
	}
}

func TestDecodeEmptyContent(t *testing.T) {
	for _, format := range []string{"toml", "json"} {
		decoded, err := decodeConfig(format, []byte("  \n"))
		if err != nil {
			t.Fatalf("decode empty %s: %v", format, err)
		}
		if len(decoded) != 0 {
			t.Fatalf("expected empty %s document, got %#v", format, decoded)
		}
	}
}

func TestResolveFormat(t *testing.T) {
	cases := []struct {
		explicit string
		livePath string
		want     string
		wantErr  bool
	}{
		{"", "/home/u/.config/codex/config.toml", "toml", false},
		{"", "/home/u/.claude/settings.json", "json", false},
		{"json", "/home/u/.config/app/config", "json", false},
		{"", "/home/u/.config/app/config", "", true},
		{"yaml", "/home/u/.config/app/config.yaml", "", true},
	}

	for _, testCase := range cases {
		got, err := resolveFormat(testCase.explicit, testCase.livePath)
		if testCase.wantErr {
			if err == nil {
				t.Fatalf("expected error for (%q, %q)", testCase.explicit, testCase.livePath)
			}
			continue
		}

		if err != nil {
			t.Fatalf("resolveFormat(%q, %q): %v", testCase.explicit, testCase.livePath, err)
		}

		if got != testCase.want {
			t.Fatalf("resolveFormat(%q, %q) = %q, want %q", testCase.explicit, testCase.livePath, got, testCase.want)
		}
	}
}

func TestParsePatternsRejectsEmptySegments(t *testing.T) {
	if _, err := parsePatterns([]string{"ui..sound"}); err == nil {
		t.Fatalf("expected an error for an empty path segment")
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

// --- daemon-level tests -----------------------------------------------------
//
// These drive sync() against real files so the parts that can destroy a config
// are covered: out-of-band edit detection, recovery from unparseable files, and
// the write-only-if-changed guard.

type syncFixture struct {
	daemon *daemon
	base   string
	live   string
	rt     string
}

func newSyncFixture(t *testing.T, format string, baseContent string, runtimeKeys ...string) *syncFixture {
	t.Helper()

	dir := t.TempDir()
	fixture := &syncFixture{
		base: filepath.Join(dir, "base."+format),
		live: filepath.Join(dir, "live."+format),
		rt:   filepath.Join(dir, "state", "overlay."+format),
	}

	writeTestFile(t, fixture.base, baseContent)

	fixture.daemon = &daemon{
		basePath:    fixture.base,
		runtimePath: fixture.rt,
		livePath:    fixture.live,
		format:      format,
		patterns:    mustPatterns(runtimeKeys...),
	}

	return fixture
}

func (f *syncFixture) sync(t *testing.T) {
	t.Helper()

	if err := f.daemon.sync(); err != nil {
		t.Fatalf("sync: %v", err)
	}
}

func (f *syncFixture) liveDoc(t *testing.T) configMap {
	t.Helper()

	decoded, err := decodeConfig(f.daemon.format, []byte(readTestFile(t, f.live)))
	if err != nil {
		t.Fatalf("decode live: %v", err)
	}

	return decoded
}

func writeTestFile(t *testing.T, path string, content string) {
	t.Helper()

	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatalf("mkdir %s: %v", filepath.Dir(path), err)
	}

	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatalf("write %s: %v", path, err)
	}
}

func readTestFile(t *testing.T, path string) string {
	t.Helper()

	content, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read %s: %v", path, err)
	}

	return string(content)
}

func TestSyncRendersBaseIntoMissingLive(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "[theme]\nname = \"catppuccin\"\n")
	fixture.sync(t)

	if value, _ := getPath(fixture.liveDoc(t), []string{"theme", "name"}); value != "catppuccin" {
		t.Fatalf("expected base to be rendered into the live file, got %#v", value)
	}

	if _, err := os.Stat(fixture.rt); !os.IsNotExist(err) {
		t.Fatalf("expected no overlay to be written when nothing was harvested")
	}
}

func TestSyncHarvestsOutOfBandEdit(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "[theme]\nname = \"catppuccin\"\n", "ui.agent_panel_sort")
	fixture.sync(t)

	// The application rewrites its own config behind the daemon's back.
	writeTestFile(t, fixture.live, readTestFile(t, fixture.live)+"\n[ui]\nagent_panel_sort = \"priority\"\n")
	fixture.sync(t)

	overlay, err := decodeConfig("toml", []byte(readTestFile(t, fixture.rt)))
	if err != nil {
		t.Fatalf("decode overlay: %v", err)
	}

	if value, _ := getPath(overlay, []string{"ui", "agent_panel_sort"}); value != "priority" {
		t.Fatalf("expected the edit to be harvested into the overlay, got %#v", value)
	}

	if value, _ := getPath(fixture.liveDoc(t), []string{"ui", "agent_panel_sort"}); value != "priority" {
		t.Fatalf("expected the harvested value to survive the re-render, got %#v", value)
	}

	// A third sync with no external change must not lose it either.
	fixture.sync(t)
	if value, _ := getPath(fixture.liveDoc(t), []string{"ui", "agent_panel_sort"}); value != "priority" {
		t.Fatalf("expected the value to persist across syncs, got %#v", value)
	}
}

func TestSyncDropsUnlistedOutOfBandEdit(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "[theme]\nname = \"catppuccin\"\n", "ui.agent_panel_sort")
	fixture.sync(t)

	writeTestFile(t, fixture.live, readTestFile(t, fixture.live)+"\n[ui]\nsidebar_width = 40\n")
	fixture.sync(t)

	if _, exists := getPath(fixture.liveDoc(t), []string{"ui", "sidebar_width"}); exists {
		t.Fatalf("expected an unlisted key to be discarded")
	}
}

func TestSyncPreserveUnknownKeepsOutOfBandEdit(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "[theme]\nname = \"catppuccin\"\n")
	fixture.daemon.preserveUnknown = true
	fixture.sync(t)

	writeTestFile(t, fixture.live, readTestFile(t, fixture.live)+"\n[ui]\nsidebar_width = 40\n")
	fixture.sync(t)

	live := fixture.liveDoc(t)
	if value, _ := getPath(live, []string{"ui", "sidebar_width"}); value != int64(40) {
		t.Fatalf("expected the undeclared key to be kept, got %#v", value)
	}

	if value, _ := getPath(live, []string{"theme", "name"}); value != "catppuccin" {
		t.Fatalf("expected the declared key to stay pinned, got %#v", value)
	}
}

func TestSyncRestoresUnparseableLive(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "[theme]\nname = \"catppuccin\"\n", "ui.agent_panel_sort")
	fixture.sync(t)

	writeTestFile(t, fixture.live, "this is not = valid toml [[[")

	if err := fixture.daemon.sync(); err != nil {
		t.Fatalf("expected sync to recover from an unparseable live file, got %v", err)
	}

	if value, _ := getPath(fixture.liveDoc(t), []string{"theme", "name"}); value != "catppuccin" {
		t.Fatalf("expected the live file to be restored from base, got %#v", value)
	}
}

func TestSyncFallsBackToLastGoodRuntime(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "[theme]\nname = \"catppuccin\"\n", "ui.agent_panel_sort")
	fixture.sync(t)

	writeTestFile(t, fixture.live, readTestFile(t, fixture.live)+"\n[ui]\nagent_panel_sort = \"priority\"\n")
	fixture.sync(t)

	// Corrupt the overlay the daemon just wrote; the in-memory copy must cover.
	writeTestFile(t, fixture.rt, "not [[ valid")

	if err := fixture.daemon.sync(); err != nil {
		t.Fatalf("expected sync to fall back to the last good overlay, got %v", err)
	}

	if value, _ := getPath(fixture.liveDoc(t), []string{"ui", "agent_panel_sort"}); value != "priority" {
		t.Fatalf("expected the last good runtime state to survive, got %#v", value)
	}
}

func TestSyncErrorsWhenOverlayCorruptWithNoLastGood(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "[theme]\nname = \"catppuccin\"\n", "ui.agent_panel_sort")
	writeTestFile(t, fixture.rt, "not [[ valid")

	if err := fixture.daemon.sync(); err == nil {
		t.Fatalf("expected an error when the overlay is corrupt and no good state is known")
	}
}

func TestSyncErrorsWhenBaseMissing(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "[theme]\nname = \"catppuccin\"\n")
	if err := os.Remove(fixture.base); err != nil {
		t.Fatalf("remove base: %v", err)
	}

	if err := fixture.daemon.sync(); err == nil {
		t.Fatalf("expected a missing base config to be an error")
	}
}

func TestSyncSkipsWriteWhenUnchanged(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "[theme]\nname = \"catppuccin\"\n")
	fixture.sync(t)

	// A rewrite renames a fresh 0600 temp file over the live path, so a mode
	// we set by hand surviving proves no write happened.
	if err := os.Chmod(fixture.live, 0o644); err != nil {
		t.Fatalf("chmod live: %v", err)
	}

	fixture.sync(t)

	info, err := os.Stat(fixture.live)
	if err != nil {
		t.Fatalf("stat live: %v", err)
	}

	if info.Mode().Perm() != 0o644 {
		t.Fatalf("expected an unchanged live file to be left alone, mode is now %v", info.Mode().Perm())
	}
}

func TestSyncJSONPreservesArrays(t *testing.T) {
	base := `{"hooks":{"SessionStart":[{"matcher":"*","hooks":[{"type":"command","timeout":10}]}]}}`
	fixture := newSyncFixture(t, "json", base, "feedbackSurveyState.*")
	fixture.sync(t)

	entries, ok := getPath(fixture.liveDoc(t), []string{"hooks", "SessionStart"})
	if !ok {
		t.Fatalf("expected hooks.SessionStart to be rendered")
	}

	list, ok := entries.([]any)
	if !ok || len(list) != 1 {
		t.Fatalf("expected a one-element array, got %#v", entries)
	}

	entry, ok := asConfigMap(list[0])
	if !ok {
		t.Fatalf("expected an object inside the array, got %#v", list[0])
	}

	if entry["matcher"] != "*" {
		t.Fatalf("expected the array element to survive, got %#v", entry["matcher"])
	}

	nested, ok := entry["hooks"].([]any)
	if !ok || len(nested) != 1 {
		t.Fatalf("expected the nested array to survive, got %#v", entry["hooks"])
	}
}

func TestWriteConfigCreatesDirectoryWithTightPermissions(t *testing.T) {
	path := filepath.Join(t.TempDir(), "nested", "deeper", "config.toml")

	if err := writeConfig(path, "key = \"value\"\n"); err != nil {
		t.Fatalf("writeConfig: %v", err)
	}

	info, err := os.Stat(path)
	if err != nil {
		t.Fatalf("stat: %v", err)
	}

	if info.Mode().Perm() != 0o600 {
		t.Fatalf("expected mode 0600, got %v", info.Mode().Perm())
	}

	if got := readTestFile(t, path); got != "key = \"value\"\n" {
		t.Fatalf("unexpected content %q", got)
	}

	// No temp files may be left behind on success.
	entries, err := os.ReadDir(filepath.Dir(path))
	if err != nil {
		t.Fatalf("readdir: %v", err)
	}

	if len(entries) != 1 {
		t.Fatalf("expected only the config file to remain, got %d entries", len(entries))
	}
}

func TestReadConfigMissingFile(t *testing.T) {
	d := &daemon{format: "toml"}
	path := filepath.Join(t.TempDir(), "absent.toml")

	doc, raw, hash, err := d.readConfig(path, true)
	if err != nil {
		t.Fatalf("expected a missing file to be tolerated, got %v", err)
	}

	if len(doc) != 0 || raw != "" || hash != "" {
		t.Fatalf("expected an empty result, got doc=%#v raw=%q hash=%q", doc, raw, hash)
	}

	if _, _, _, err := d.readConfig(path, false); err == nil {
		t.Fatalf("expected a missing required file to be an error")
	}
}

func TestDeepCopyValueCopiesSlices(t *testing.T) {
	source := configMap{
		"hooks": []any{
			configMap{"command": "original"},
			"plain",
		},
	}

	copied := deepCopyMap(source)

	sourceList := source["hooks"].([]any)
	sourceEntry := sourceList[0].(configMap)
	sourceEntry["command"] = "mutated"
	sourceList[1] = "changed"

	copiedList, ok := copied["hooks"].([]any)
	if !ok {
		t.Fatalf("expected the slice to be copied, got %#v", copied["hooks"])
	}

	copiedEntry, ok := asConfigMap(copiedList[0])
	if !ok {
		t.Fatalf("expected an object inside the copied slice")
	}

	if copiedEntry["command"] != "original" {
		t.Fatalf("expected the copy to be independent, got %#v", copiedEntry["command"])
	}

	if copiedList[1] != "plain" {
		t.Fatalf("expected slice elements to be copied, got %#v", copiedList[1])
	}
}

// Regression: a restarted daemon must recognise the live file it wrote in a
// previous run. Without a persisted stamp it treats its own output as an
// out-of-band edit and, under preserve-unknown, promotes the previous
// generation's base into the overlay where it never goes away.
func TestSyncDoesNotHarvestOwnOutputAfterRestart(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "onboarding = false\n[theme]\nname = \"catppuccin\"\n")
	fixture.daemon.preserveUnknown = true
	fixture.daemon.loadStamp()
	fixture.sync(t)

	// A rebuild empties the declarative base, then restarts the unit: a fresh
	// daemon over the same paths.
	writeTestFile(t, fixture.base, "")
	restarted := &daemon{
		basePath:        fixture.base,
		runtimePath:     fixture.rt,
		livePath:        fixture.live,
		format:          "toml",
		preserveUnknown: true,
	}
	restarted.loadStamp()

	if err := restarted.sync(); err != nil {
		t.Fatalf("sync after restart: %v", err)
	}

	if content, err := os.ReadFile(fixture.rt); err == nil && len(content) > 0 {
		t.Fatalf("expected no overlay after restart, got %q", content)
	}

	live := fixture.liveDoc(t)
	if _, exists := live["onboarding"]; exists {
		t.Fatalf("expected the retired declarative key to be gone, got %#v", live["onboarding"])
	}

	if _, exists := live["theme"]; exists {
		t.Fatalf("expected the retired theme table to be gone, got %#v", live["theme"])
	}
}

// The stamp must not suppress harvesting of edits the application really made.
func TestSyncStillHarvestsRealEditAfterRestart(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "[theme]\nname = \"catppuccin\"\n")
	fixture.daemon.preserveUnknown = true
	fixture.daemon.loadStamp()
	fixture.sync(t)

	writeTestFile(t, fixture.live, readTestFile(t, fixture.live)+"\n[ui]\nsidebar_width = 40\n")

	restarted := &daemon{
		basePath:        fixture.base,
		runtimePath:     fixture.rt,
		livePath:        fixture.live,
		format:          "toml",
		preserveUnknown: true,
	}
	restarted.loadStamp()

	if err := restarted.sync(); err != nil {
		t.Fatalf("sync after restart: %v", err)
	}

	if value, _ := getPath(fixture.liveDoc(t), []string{"ui", "sidebar_width"}); value != int64(40) {
		t.Fatalf("expected a genuine edit to still be harvested, got %#v", value)
	}
}

func TestStampRoundTrip(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "[theme]\nname = \"catppuccin\"\n")
	fixture.sync(t)

	stamp := readTestFile(t, fixture.daemon.stampPath())
	if len(stamp) == 0 {
		t.Fatalf("expected a stamp to be written")
	}

	fresh := &daemon{runtimePath: fixture.rt}
	fresh.loadStamp()

	if fresh.lastWrittenLive != fixture.daemon.lastWrittenLive {
		t.Fatalf("expected the stamp to round trip, got %q want %q", fresh.lastWrittenLive, fixture.daemon.lastWrittenLive)
	}
}

func TestSyncRunsOnChangeAfterWrite(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "[theme]\nname = \"catppuccin\"\n")
	marker := filepath.Join(t.TempDir(), "reloaded")
	// A shell builtin only, so the test does not depend on coreutils.
	fixture.daemon.onChange = ": > " + marker

	fixture.sync(t)

	if _, err := os.Stat(marker); err != nil {
		t.Fatalf("expected the on-change command to run after a write: %v", err)
	}

	if err := os.Remove(marker); err != nil {
		t.Fatalf("remove marker: %v", err)
	}

	// A no-op sync must not re-trigger it.
	fixture.sync(t)

	if _, err := os.Stat(marker); !os.IsNotExist(err) {
		t.Fatalf("expected no on-change command when the live file was unchanged")
	}
}

func TestSyncToleratesFailingOnChange(t *testing.T) {
	fixture := newSyncFixture(t, "toml", "[theme]\nname = \"catppuccin\"\n")
	fixture.daemon.onChange = "exit 1"

	if err := fixture.daemon.sync(); err != nil {
		t.Fatalf("expected a failing on-change command to be non-fatal, got %v", err)
	}

	if value, _ := getPath(fixture.liveDoc(t), []string{"theme", "name"}); value != "catppuccin" {
		t.Fatalf("expected the config to be written regardless, got %#v", value)
	}
}

func TestRunOnChangeSkippedWhenUnset(t *testing.T) {
	d := &daemon{}
	// Must be a no-op rather than spawning an empty shell.
	d.runOnChange()
}
