package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"flag"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/BurntSushi/toml"
)

type daemon struct {
	basePath    string
	runtimePath string
	livePath    string
	interval    time.Duration

	lastBaseHash       string
	lastRuntimeHash    string
	lastLiveHash       string
	lastWrittenRuntime string
	lastWrittenLive    string

	lastGoodRuntime configMap
}

type configMap map[string]any

func main() {
	basePath := flag.String("base", "", "Path to the declarative Codex base config")
	runtimePath := flag.String("runtime", "", "Path to the daemon-managed Codex runtime overlay")
	livePath := flag.String("live", "", "Path to the live Codex config file")
	interval := flag.Duration("interval", 60*time.Second, "Polling interval")
	flag.Parse()

	if *basePath == "" || *runtimePath == "" || *livePath == "" {
		flag.Usage()
		os.Exit(2)
	}

	log.SetFlags(log.LstdFlags | log.Lmicroseconds)

	d := &daemon{
		basePath:    *basePath,
		runtimePath: *runtimePath,
		livePath:    *livePath,
		interval:    *interval,
	}

	if err := d.run(); err != nil {
		log.Fatalf("daemon exited: %v", err)
	}
}

func (d *daemon) run() error {
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
	baseDoc, _, baseHash, err := readConfig(d.basePath, false)
	if err != nil {
		return fmt.Errorf("read base config: %w", err)
	}

	runtimeDoc, runtimeRaw, runtimeHash, runtimeErr := readConfig(d.runtimePath, true)
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

	liveDoc, liveRaw, liveHash, liveErr := readConfig(d.livePath, true)
	if liveErr != nil {
		log.Printf("live config invalid, will restore merged config: %v", liveErr)
		liveDoc = configMap{}
		liveRaw = ""
		liveHash = ""
	}

	liveChangedOutside := liveHash != "" && liveHash != d.lastLiveHash && liveHash != d.lastWrittenLive
	if liveChangedOutside {
		extracted := extractRuntimeState(liveDoc)
		extractedRaw, err := marshalConfig(extracted)
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
			d.lastWrittenRuntime = runtimeHash
			d.lastGoodRuntime = deepCopyMap(extracted)
			log.Printf("updated runtime overlay from live Codex config")
		}
	}

	merged := mergeConfig(baseDoc, runtimeDoc)
	mergedRaw, err := marshalConfig(merged)
	if err != nil {
		return fmt.Errorf("marshal merged config: %w", err)
	}

	if mergedRaw != liveRaw {
		if err := writeConfig(d.livePath, mergedRaw); err != nil {
			return fmt.Errorf("write live config: %w", err)
		}
		liveHash = hashContent(mergedRaw)
		d.lastWrittenLive = liveHash
		log.Printf("wrote merged Codex config")
	}

	if runtimeHash == "" {
		runtimeHash = hashContent(runtimeRaw)
	}

	d.lastBaseHash = baseHash
	d.lastRuntimeHash = runtimeHash
	d.lastLiveHash = liveHash

	return nil
}

func readConfig(path string, allowMissing bool) (configMap, string, string, error) {
	content, err := os.ReadFile(path)
	if err != nil {
		if allowMissing && errors.Is(err, os.ErrNotExist) {
			return configMap{}, "", "", nil
		}
		return nil, "", "", err
	}

	var decoded configMap
	if len(bytes.TrimSpace(content)) == 0 {
		decoded = configMap{}
	} else if err := toml.Unmarshal(content, &decoded); err != nil {
		return nil, "", "", err
	}

	raw := string(content)
	return decoded, raw, hashContent(raw), nil
}

func extractRuntimeState(doc configMap) configMap {
	projects := getProjectsTable(doc)
	if len(projects) == 0 {
		return configMap{}
	}

	runtimeProjects := make(configMap)
	for projectPath, projectValue := range projects {
		projectTable, ok := asConfigMap(projectValue)
		if !ok {
			continue
		}

		trustLevel, ok := projectTable["trust_level"].(string)
		if !ok {
			continue
		}

		trustLevel = strings.TrimSpace(trustLevel)
		if trustLevel == "" {
			continue
		}

		runtimeProjects[projectPath] = configMap{
			"trust_level": trustLevel,
		}
	}

	if len(runtimeProjects) == 0 {
		return configMap{}
	}

	return configMap{
		"projects": runtimeProjects,
	}
}

func mergeConfig(base, runtime configMap) configMap {
	merged := deepCopyMap(base)
	runtimeProjects := getProjectsTable(runtime)
	if len(runtimeProjects) == 0 {
		return merged
	}

	var mergedProjects configMap
	if existing, ok := merged["projects"]; ok {
		projectTable, ok := asConfigMap(existing)
		if !ok {
			return merged
		}
		mergedProjects = deepCopyMap(projectTable)
	} else {
		mergedProjects = configMap{}
	}

	for projectPath, projectValue := range runtimeProjects {
		runtimeProject, ok := asConfigMap(projectValue)
		if !ok {
			continue
		}

		runtimeTrust, ok := runtimeProject["trust_level"].(string)
		if !ok {
			continue
		}

		if existingValue, ok := mergedProjects[projectPath]; ok {
			existingProject, ok := asConfigMap(existingValue)
			if !ok {
				continue
			}
			if _, exists := existingProject["trust_level"]; exists {
				continue
			}

			nextProject := deepCopyMap(existingProject)
			nextProject["trust_level"] = runtimeTrust
			mergedProjects[projectPath] = nextProject
			continue
		}

		mergedProjects[projectPath] = configMap{
			"trust_level": runtimeTrust,
		}
	}

	if len(mergedProjects) != 0 {
		merged["projects"] = mergedProjects
	}

	return merged
}

func marshalConfig(doc configMap) (string, error) {
	var buffer bytes.Buffer
	encoder := toml.NewEncoder(&buffer)
	if err := encoder.Encode(doc); err != nil {
		return "", err
	}

	return buffer.String(), nil
}

func writeConfig(path string, content string) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}

	tempFile, err := os.CreateTemp(dir, ".codex-config-merge-*")
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

func getProjectsTable(doc configMap) configMap {
	projectsValue, ok := doc["projects"]
	if !ok {
		return nil
	}

	projects, ok := asConfigMap(projectsValue)
	if !ok {
		return nil
	}

	return projects
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

	copy := make(configMap, len(source))
	for key, value := range source {
		copy[key] = deepCopyValue(value)
	}

	return copy
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
