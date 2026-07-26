package agent

import (
	"fmt"
	"os"
	"strings"
	"time"
)

// Profile maps a Home Assistant-facing profile ID to a systemd template
// instance name.
type Profile struct {
	ID       string // HA-facing id, e.g. "brad"
	Instance string // systemd template instance, e.g. "game-brad"
}

// Config is the fully-resolved agent configuration.
type Config struct {
	Broker          string // host:port of the MQTT broker
	Username        string
	Password        string
	ClientID        string
	TopicPrefix     string // e.g. "romeo/gamestream"
	DiscoveryPrefix string // e.g. "homeassistant"
	NodeID          string // e.g. "romeo_gamestream"
	DeviceName      string // e.g. "Romeo Game Streaming"
	SocketPath      string // unix socket for `notify`
	UnitTemplate    string // e.g. "gamestream@%s.service"
	StartTimeout    time.Duration
	Profiles        []Profile
}

// Unit returns the systemd unit name for a profile using the configured
// template.
func (c Config) Unit(p Profile) string {
	return fmt.Sprintf(c.UnitTemplate, p.Instance)
}

// Topic helpers. Keeping these on Config makes the topic scheme a single source
// of truth shared by discovery, publishing and subscribing.

func (c Config) CommandTopic(p Profile) string     { return c.TopicPrefix + "/" + p.ID + "/set" }
func (c Config) SwitchStateTopic(p Profile) string { return c.TopicPrefix + "/" + p.ID + "/switch" }
func (c Config) SensorStateTopic(p Profile) string { return c.TopicPrefix + "/" + p.ID + "/state" }
func (c Config) AvailabilityTopic() string         { return c.TopicPrefix + "/availability" }

func (c Config) SwitchDiscoveryTopic(p Profile) string {
	return fmt.Sprintf("%s/switch/%s_%s/config", c.DiscoveryPrefix, c.NodeID, p.ID)
}
func (c Config) SensorDiscoveryTopic(p Profile) string {
	return fmt.Sprintf("%s/sensor/%s_%s/config", c.DiscoveryPrefix, c.NodeID, p.ID)
}

// defaultConfig returns the config with all defaults applied but no secrets.
func defaultConfig() Config {
	return Config{
		Broker:          "192.168.3.6:1883",
		Username:        "gamestream",
		ClientID:        "gamestream-agent",
		TopicPrefix:     "romeo/gamestream",
		DiscoveryPrefix: "homeassistant",
		NodeID:          "romeo_gamestream",
		DeviceName:      "Romeo Game Streaming",
		SocketPath:      "/run/gamestream-agent/notify.sock",
		UnitTemplate:    "gamestream@%s.service",
		StartTimeout:    120 * time.Second,
	}
}

// LoadConfig builds a Config from environment variables read via getenv,
// applying defaults. readFile is used to resolve GAMESTREAM_MQTT_PASSWORD_FILE
// (pass os.ReadFile for production). Both are injected for testability.
func LoadConfig(getenv func(string) string, readFile func(string) ([]byte, error)) (Config, error) {
	c := defaultConfig()

	if v := getenv("GAMESTREAM_MQTT_BROKER"); v != "" {
		c.Broker = v
	}
	if v := getenv("GAMESTREAM_MQTT_USERNAME"); v != "" {
		c.Username = v
	}
	if v := getenv("GAMESTREAM_MQTT_PASSWORD"); v != "" {
		c.Password = v
	}
	if v := getenv("GAMESTREAM_MQTT_PASSWORD_FILE"); v != "" {
		b, err := readFile(v)
		if err != nil {
			return Config{}, fmt.Errorf("reading GAMESTREAM_MQTT_PASSWORD_FILE: %w", err)
		}
		c.Password = strings.TrimSpace(string(b))
	}
	if v := getenv("GAMESTREAM_CLIENT_ID"); v != "" {
		c.ClientID = v
	}
	if v := getenv("GAMESTREAM_TOPIC_PREFIX"); v != "" {
		c.TopicPrefix = strings.TrimRight(v, "/")
	}
	if v := getenv("GAMESTREAM_DISCOVERY_PREFIX"); v != "" {
		c.DiscoveryPrefix = strings.TrimRight(v, "/")
	}
	if v := getenv("GAMESTREAM_NODE_ID"); v != "" {
		c.NodeID = v
	}
	if v := getenv("GAMESTREAM_DEVICE_NAME"); v != "" {
		c.DeviceName = v
	}
	if v := getenv("GAMESTREAM_SOCKET"); v != "" {
		c.SocketPath = v
	}
	if v := getenv("GAMESTREAM_UNIT_TEMPLATE"); v != "" {
		c.UnitTemplate = v
	}
	if v := getenv("GAMESTREAM_START_TIMEOUT"); v != "" {
		d, err := time.ParseDuration(v)
		if err != nil {
			return Config{}, fmt.Errorf("parsing GAMESTREAM_START_TIMEOUT: %w", err)
		}
		c.StartTimeout = d
	}

	profiles, err := ParseProfiles(getenv("GAMESTREAM_PROFILES"))
	if err != nil {
		return Config{}, err
	}
	c.Profiles = profiles

	return c, nil
}

// LoadConfigFromEnv is the production entrypoint using the real environment.
func LoadConfigFromEnv() (Config, error) {
	return LoadConfig(os.Getenv, os.ReadFile)
}

// ParseProfiles parses the GAMESTREAM_PROFILES value of the form
// "id=instance,id=instance" into an ordered slice of Profiles.
func ParseProfiles(s string) ([]Profile, error) {
	s = strings.TrimSpace(s)
	if s == "" {
		return nil, nil
	}
	var profiles []Profile
	for _, part := range strings.Split(s, ",") {
		part = strings.TrimSpace(part)
		if part == "" {
			continue
		}
		id, inst, ok := strings.Cut(part, "=")
		id, inst = strings.TrimSpace(id), strings.TrimSpace(inst)
		if !ok || id == "" || inst == "" || strings.Contains(inst, "=") {
			return nil, fmt.Errorf("invalid profile entry %q, want id=instance", part)
		}
		profiles = append(profiles, Profile{ID: id, Instance: inst})
	}
	return profiles, nil
}

// Validate checks that the config is usable for `serve`.
func (c Config) Validate() error {
	if c.Broker == "" {
		return fmt.Errorf("mqtt broker is required")
	}
	if !strings.Contains(c.UnitTemplate, "%s") {
		return fmt.Errorf("unit template %q must contain %%s", c.UnitTemplate)
	}
	if len(c.Profiles) == 0 {
		return fmt.Errorf("at least one profile is required")
	}
	seenID := map[string]bool{}
	seenInst := map[string]bool{}
	for _, p := range c.Profiles {
		if seenID[p.ID] {
			return fmt.Errorf("duplicate profile id %q", p.ID)
		}
		if seenInst[p.Instance] {
			return fmt.Errorf("duplicate profile instance %q", p.Instance)
		}
		seenID[p.ID] = true
		seenInst[p.Instance] = true
	}
	return nil
}

// ProfileIDs returns the ordered list of profile IDs.
func (c Config) ProfileIDs() []string {
	ids := make([]string, len(c.Profiles))
	for i, p := range c.Profiles {
		ids[i] = p.ID
	}
	return ids
}
