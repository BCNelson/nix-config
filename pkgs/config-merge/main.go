// config-merge keeps a declarative, read-only base config (typically a
// /nix/store path) merged with the runtime state an application writes for
// itself, producing a single live config file the application can freely
// rewrite in place.
//
// Applications that persist their own settings cannot read their config from a
// read-only store symlink. Instead of giving up the declarative config, this
// daemon owns the live file: it renders base into it, harvests the
// application's own writes into a runtime overlay, and replays that overlay on
// the next render. Keys the base declares always win, so Nix stays the source
// of truth for everything it names.
package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/BurntSushi/toml"
)

// onChangeTimeout bounds the post-write hook so a hung reload cannot wedge the
// reconcile loop.
const onChangeTimeout = 30 * time.Second

type configMap map[string]any

// keyPattern is a dotted key path split into segments, where "*" matches any
// key at that level (e.g. projects.*.trust_level).
type keyPattern []string

type stringList []string

func (s *stringList) String() string {
	return strings.Join(*s, ",")
}

func (s *stringList) Set(value string) error {
	*s = append(*s, value)
	return nil
}

type daemon struct {
	basePath        string
	runtimePath     string
	livePath        string
	format          string
	patterns        []keyPattern
	preserveUnknown bool
	onChange        string
	interval        time.Duration

	lastBaseHash    string
	lastRuntimeHash string
	lastLiveHash    string
	lastWrittenLive string

	lastGoodRuntime configMap
}

func main() {
	basePath := flag.String("base", "", "Path to the declarative base config")
	runtimePath := flag.String("runtime", "", "Path to the daemon-managed runtime overlay")
	livePath := flag.String("live", "", "Path to the live config file the application reads and writes")
	format := flag.String("format", "", "Config format: toml or json (default: inferred from --live)")
	preserveUnknown := flag.Bool("preserve-unknown", false, "Carry over every key the base does not declare, instead of only --runtime-key paths")
	onChange := flag.String("on-change", "", "Shell command run after the live config is rewritten, e.g. to make a running application reload it")
	interval := flag.Duration("interval", 60*time.Second, "Polling interval")

	var runtimeKeys stringList
	flag.Var(&runtimeKeys, "runtime-key", "Dotted key path the application owns at runtime; '*' matches any key. Repeatable.")

	flag.Parse()

	if *basePath == "" || *runtimePath == "" || *livePath == "" {
		flag.Usage()
		os.Exit(2)
	}

	resolvedFormat, err := resolveFormat(*format, *livePath)
	if err != nil {
		log.Fatalf("%v", err)
	}

	patterns, err := parsePatterns(runtimeKeys)
	if err != nil {
		log.Fatalf("%v", err)
	}

	log.SetFlags(log.LstdFlags | log.Lmicroseconds)

	d := &daemon{
		basePath:        *basePath,
		runtimePath:     *runtimePath,
		livePath:        *livePath,
		format:          resolvedFormat,
		patterns:        patterns,
		preserveUnknown: *preserveUnknown,
		onChange:        *onChange,
		interval:        *interval,
	}

	if err := d.run(); err != nil {
		log.Fatalf("daemon exited: %v", err)
	}
}

// runOnChange notifies a running application that its config was rewritten.
// Applications that cache config at startup - herdr keeps it in the server
// process - do not notice the daemon editing the file underneath them. A
// failure here is never fatal: the config on disk is already correct, only the
// live reload was missed.
func (d *daemon) runOnChange() {
	if d.onChange == "" {
		return
	}

	ctx, cancel := context.WithTimeout(context.Background(), onChangeTimeout)
	defer cancel()

	output, err := exec.CommandContext(ctx, "sh", "-c", d.onChange).CombinedOutput()
	if err != nil {
		log.Printf("on-change command failed: %v: %s", err, bytes.TrimSpace(output))
		return
	}

	log.Printf("ran on-change command")
}

// stampPath holds the hash of the last live file this daemon wrote. Without it
// the daemon restarts with no memory of its own output and misreads the live
// file as an out-of-band edit, harvesting the previous generation's base into
// the overlay - which under preserve-unknown silently promotes declarative
// values to runtime state and freezes them there.
func (d *daemon) stampPath() string {
	return d.runtimePath + ".stamp"
}

func (d *daemon) loadStamp() {
	content, err := os.ReadFile(d.stampPath())
	if err != nil {
		if !errors.Is(err, os.ErrNotExist) {
			log.Printf("read write stamp: %v", err)
		}
		return
	}

	d.lastWrittenLive = strings.TrimSpace(string(content))
}

func (d *daemon) saveStamp(hash string) {
	if err := writeConfig(d.stampPath(), hash+"\n"); err != nil {
		log.Printf("write stamp: %v", err)
	}
}

func (d *daemon) run() error {
	d.loadStamp()

	ticker := time.NewTicker(d.interval)
	defer ticker.Stop()

	for {
		if err := d.sync(); err != nil {
			log.Printf("sync failed: %v", err)
		}

		<-ticker.C
	}
}

func (d *daemon) sync() error {
	baseDoc, _, baseHash, err := d.readConfig(d.basePath, false)
	if err != nil {
		return fmt.Errorf("read base config: %w", err)
	}

	runtimeDoc, runtimeRaw, runtimeHash, runtimeErr := d.readConfig(d.runtimePath, true)
	if runtimeErr != nil {
		if d.lastGoodRuntime != nil {
			log.Printf("runtime overlay invalid, using last good state: %v", runtimeErr)
			runtimeDoc = deepCopyMap(d.lastGoodRuntime)
			runtimeRaw = ""
			runtimeHash = d.lastRuntimeHash
		} else {
			return fmt.Errorf("read runtime overlay: %w", runtimeErr)
		}
	} else {
		d.lastGoodRuntime = deepCopyMap(runtimeDoc)
	}

	liveDoc, liveRaw, liveHash, liveErr := d.readConfig(d.livePath, true)
	if liveErr != nil {
		log.Printf("live config invalid, will restore merged config: %v", liveErr)
		liveDoc = configMap{}
		liveRaw = ""
		liveHash = ""
	}

	liveChangedOutside := liveHash != "" && liveHash != d.lastLiveHash && liveHash != d.lastWrittenLive
	if liveChangedOutside {
		extracted := d.extract(liveDoc, baseDoc)
		extractedRaw, err := encodeConfig(d.format, extracted)
		if err != nil {
			return fmt.Errorf("marshal extracted runtime: %w", err)
		}

		if extractedRaw != runtimeRaw {
			if err := writeConfig(d.runtimePath, extractedRaw); err != nil {
				return fmt.Errorf("write runtime overlay: %w", err)
			}
			runtimeDoc = extracted
			runtimeRaw = extractedRaw
			runtimeHash = hashContent(extractedRaw)
			d.lastGoodRuntime = deepCopyMap(extracted)
			log.Printf("updated runtime overlay from live config")
		}
	}

	merged := d.merge(baseDoc, runtimeDoc)
	mergedRaw, err := encodeConfig(d.format, merged)
	if err != nil {
		return fmt.Errorf("marshal merged config: %w", err)
	}

	if liveChangedOutside {
		if removed := removedKeyPaths(liveDoc, merged); len(removed) > 0 {
			log.Printf("discarding unpreserved live config keys: %s", strings.Join(removed, ", "))
		}
	}

	if mergedRaw != liveRaw {
		if err := writeConfig(d.livePath, mergedRaw); err != nil {
			return fmt.Errorf("write live config: %w", err)
		}
		liveHash = hashContent(mergedRaw)
		d.lastWrittenLive = liveHash
		d.saveStamp(liveHash)
		log.Printf("wrote merged config to %s", d.livePath)
		d.runOnChange()
	}

	if runtimeHash == "" {
		runtimeHash = hashContent(runtimeRaw)
	}

	d.lastBaseHash = baseHash
	d.lastRuntimeHash = runtimeHash
	d.lastLiveHash = liveHash

	return nil
}

func (d *daemon) readConfig(path string, allowMissing bool) (configMap, string, string, error) {
	content, err := os.ReadFile(path)
	if err != nil {
		if allowMissing && errors.Is(err, os.ErrNotExist) {
			return configMap{}, "", "", nil
		}
		return nil, "", "", err
	}

	decoded, err := decodeConfig(d.format, content)
	if err != nil {
		return nil, "", "", err
	}

	raw := string(content)
	return decoded, raw, hashContent(raw), nil
}

func resolveFormat(explicit string, livePath string) (string, error) {
	if explicit != "" {
		switch explicit {
		case "toml", "json":
			return explicit, nil
		default:
			return "", fmt.Errorf("unsupported format %q: want toml or json", explicit)
		}
	}

	switch strings.ToLower(filepath.Ext(livePath)) {
	case ".toml":
		return "toml", nil
	case ".json":
		return "json", nil
	default:
		return "", fmt.Errorf("cannot infer format from %q: pass --format", livePath)
	}
}

func parsePatterns(keys []string) ([]keyPattern, error) {
	patterns := make([]keyPattern, 0, len(keys))
	for _, key := range keys {
		segments := strings.Split(key, ".")
		for _, segment := range segments {
			if segment == "" {
				return nil, fmt.Errorf("invalid runtime key %q: empty path segment", key)
			}
		}
		patterns = append(patterns, keyPattern(segments))
	}

	return patterns, nil
}

func decodeConfig(format string, content []byte) (configMap, error) {
	if len(bytes.TrimSpace(content)) == 0 {
		return configMap{}, nil
	}

	var decoded configMap
	switch format {
	case "toml":
		if err := toml.Unmarshal(content, &decoded); err != nil {
			return nil, err
		}
	case "json":
		if err := json.Unmarshal(content, &decoded); err != nil {
			return nil, err
		}
	default:
		return nil, fmt.Errorf("unsupported format %q", format)
	}

	if decoded == nil {
		decoded = configMap{}
	}

	return decoded, nil
}

func encodeConfig(format string, doc configMap) (string, error) {
	switch format {
	case "toml":
		var buffer bytes.Buffer
		if err := toml.NewEncoder(&buffer).Encode(doc); err != nil {
			return "", err
		}
		return buffer.String(), nil
	case "json":
		encoded, err := json.MarshalIndent(doc, "", "  ")
		if err != nil {
			return "", err
		}
		return string(encoded) + "\n", nil
	default:
		return "", fmt.Errorf("unsupported format %q", format)
	}
}

// expandPattern resolves a pattern against a document into the concrete key
// paths it matches. Wildcards only expand over keys that actually exist, so an
// absent path yields nothing rather than an error.
func expandPattern(doc configMap, pattern keyPattern) [][]string {
	var matches [][]string

	var walk func(node configMap, index int, prefix []string)
	walk = func(node configMap, index int, prefix []string) {
		segment := pattern[index]
		last := index == len(pattern)-1

		descend := func(key string, value any) {
			path := append(append([]string(nil), prefix...), key)
			if last {
				matches = append(matches, path)
				return
			}
			if child, ok := asConfigMap(value); ok {
				walk(child, index+1, path)
			}
		}

		if segment == "*" {
			for key, value := range node {
				descend(key, value)
			}
			return
		}

		if value, ok := node[segment]; ok {
			descend(segment, value)
		}
	}

	if len(pattern) == 0 {
		return nil
	}

	walk(doc, 0, nil)
	return matches
}

func getPath(doc configMap, path []string) (any, bool) {
	node := doc
	for index, segment := range path {
		value, ok := node[segment]
		if !ok {
			return nil, false
		}

		if index == len(path)-1 {
			return value, true
		}

		child, ok := asConfigMap(value)
		if !ok {
			return nil, false
		}
		node = child
	}

	return nil, false
}

func setPath(doc configMap, path []string, value any) {
	node := doc
	for index, segment := range path {
		if index == len(path)-1 {
			node[segment] = value
			return
		}

		child, ok := asConfigMap(node[segment])
		if !ok {
			child = configMap{}
		}
		node[segment] = child
		node = child
	}
}

// extract harvests the parts of the live config worth carrying into the next
// render: everything the base does not declare in preserve-unknown mode, or
// just the explicitly enumerated runtime keys otherwise.
func (d *daemon) extract(live, base configMap) configMap {
	if d.preserveUnknown {
		return subtractDeclared(live, base)
	}

	return extractRuntimeState(live, d.patterns)
}

// merge layers the runtime overlay under the base. Either way the base wins
// wherever it declares a value; the modes differ only in how much of the
// overlay is eligible to apply.
func (d *daemon) merge(base, runtime configMap) configMap {
	if d.preserveUnknown {
		return overlayUndeclared(base, runtime)
	}

	return mergeConfig(base, runtime, d.patterns)
}

// subtractDeclared returns the parts of doc that declared does not cover.
// Tables present in both are compared key by key, so an application adding one
// field to a table the base also touches keeps that field.
func subtractDeclared(doc, declared configMap) configMap {
	remainder := configMap{}

	for key, value := range doc {
		declaredValue, exists := declared[key]
		if !exists {
			remainder[key] = deepCopyValue(value)
			continue
		}

		child, isTable := asConfigMap(value)
		declaredChild, declaredIsTable := asConfigMap(declaredValue)
		if !isTable || !declaredIsTable {
			// The base declares this path outright, so it wins and there is
			// nothing to carry over.
			continue
		}

		if nested := subtractDeclared(child, declaredChild); len(nested) > 0 {
			remainder[key] = nested
		}
	}

	return remainder
}

// overlayUndeclared merges runtime into base without a key allowlist, keeping
// the base's value at every path it declares.
func overlayUndeclared(base, runtime configMap) configMap {
	merged := deepCopyMap(base)

	for key, value := range runtime {
		baseValue, declared := merged[key]
		if !declared {
			merged[key] = deepCopyValue(value)
			continue
		}

		child, isTable := asConfigMap(value)
		baseChild, baseIsTable := asConfigMap(baseValue)
		if isTable && baseIsTable {
			merged[key] = overlayUndeclared(baseChild, child)
		}
	}

	return merged
}

// extractRuntimeState pulls just the runtime-owned keys out of the live config
// so the rest of the live file (everything the base already declares) is not
// duplicated into the overlay.
func extractRuntimeState(doc configMap, patterns []keyPattern) configMap {
	extracted := configMap{}

	for _, pattern := range patterns {
		for _, path := range expandPattern(doc, pattern) {
			value, ok := getPath(doc, path)
			if !ok {
				continue
			}
			setPath(extracted, path, deepCopyValue(value))
		}
	}

	return extracted
}

// mergeConfig layers the runtime overlay under the base: a runtime key is only
// applied where the base does not already declare that exact path, so the
// declarative config always wins.
func mergeConfig(base, runtime configMap, patterns []keyPattern) configMap {
	merged := deepCopyMap(base)

	for _, pattern := range patterns {
		for _, path := range expandPattern(runtime, pattern) {
			value, ok := getPath(runtime, path)
			if !ok {
				continue
			}

			if _, declared := getPath(merged, path); declared {
				continue
			}

			setPath(merged, path, deepCopyValue(value))
		}
	}

	return merged
}

// removedKeyPaths reports leaf keys present in the application's live config
// but absent from the config that will replace it. Values are deliberately not
// included: config files commonly contain credentials, while key paths are
// enough to identify settings that need to be added to the runtime allowlist.
func removedKeyPaths(live, replacement configMap) []string {
	var removed []string

	var collect func(value any, path []string)
	collect = func(value any, path []string) {
		if table, ok := asConfigMap(value); ok {
			if len(table) == 0 {
				removed = append(removed, strings.Join(path, "."))
				return
			}

			for key, child := range table {
				collect(child, append(append([]string(nil), path...), key))
			}
			return
		}

		removed = append(removed, strings.Join(path, "."))
	}

	var walk func(liveTable, replacementTable configMap, prefix []string)
	walk = func(liveTable, replacementTable configMap, prefix []string) {
		for key, liveValue := range liveTable {
			path := append(append([]string(nil), prefix...), key)
			replacementValue, exists := replacementTable[key]
			if !exists {
				collect(liveValue, path)
				continue
			}

			liveChild, liveIsTable := asConfigMap(liveValue)
			replacementChild, replacementIsTable := asConfigMap(replacementValue)
			if liveIsTable && replacementIsTable {
				walk(liveChild, replacementChild, path)
			} else if liveIsTable && !replacementIsTable {
				collect(liveValue, path)
			}
		}
	}

	walk(live, replacement, nil)
	sort.Strings(removed)
	return removed
}

func writeConfig(path string, content string) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}

	tempFile, err := os.CreateTemp(dir, ".config-merge-*")
	if err != nil {
		return err
	}

	tempPath := tempFile.Name()
	defer func() {
		_ = os.Remove(tempPath)
	}()

	if _, err := tempFile.WriteString(content); err != nil {
		_ = tempFile.Close()
		return err
	}

	if err := tempFile.Chmod(0o600); err != nil {
		_ = tempFile.Close()
		return err
	}

	if err := tempFile.Close(); err != nil {
		return err
	}

	return os.Rename(tempPath, path)
}

func asConfigMap(value any) (configMap, bool) {
	switch typed := value.(type) {
	case configMap:
		return typed, true
	case map[string]any:
		return configMap(typed), true
	default:
		return nil, false
	}
}

func deepCopyMap(source configMap) configMap {
	if source == nil {
		return configMap{}
	}

	copied := make(configMap, len(source))
	for key, value := range source {
		copied[key] = deepCopyValue(value)
	}

	return copied
}

func deepCopyValue(value any) any {
	switch typed := value.(type) {
	case configMap:
		return deepCopyMap(typed)
	case map[string]any:
		return deepCopyMap(configMap(typed))
	case []any:
		next := make([]any, len(typed))
		for index, item := range typed {
			next[index] = deepCopyValue(item)
		}
		return next
	default:
		return typed
	}
}

func hashContent(content string) string {
	sum := sha256.Sum256([]byte(content))
	return hex.EncodeToString(sum[:])
}
