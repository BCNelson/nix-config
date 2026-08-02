package main

import "embed"

//go:embed templates
var templateFS embed.FS

// Template describes a built-in development environment template.
type Template struct {
	Name        string   // short name used in env_create (e.g. "go", "rust")
	Description string   // human-readable description
	Indicators  []string // files whose presence signals this project type
}

// builtinTemplates lists all embedded devenv templates. Order matters for
// auto-detection: the first match wins when multiple indicators exist.
var builtinTemplates = []Template{
	{Name: "go", Description: "Go development environment", Indicators: []string{"go.mod"}},
	{Name: "rust", Description: "Rust development environment", Indicators: []string{"Cargo.toml"}},
	{Name: "python", Description: "Python development environment", Indicators: []string{"pyproject.toml", "setup.py", "requirements.txt", "uv.lock"}},
	{Name: "node-pnpm", Description: "Node.js with pnpm", Indicators: []string{"pnpm-lock.yaml"}},
	{Name: "node-yarn", Description: "Node.js with yarn", Indicators: []string{"yarn.lock"}},
	{Name: "node-npm", Description: "Node.js with npm", Indicators: []string{"package-lock.json"}},
}

// wellKnownRepos maps friendly names to git URLs for repositories that already
// contain their own devenv configuration. No template injection is needed for
// these — they are cloned as-is.
var wellKnownRepos = map[string]string{
	// Placeholder entries — add real template repos here.
	// "devenv-go":   "https://github.com/example/devenv-go-template",
}

// templateByName returns the Template with the given name, or nil.
func templateByName(name string) *Template {
	for i := range builtinTemplates {
		if builtinTemplates[i].Name == name {
			return &builtinTemplates[i]
		}
	}
	return nil
}
