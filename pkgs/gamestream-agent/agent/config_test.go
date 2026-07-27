package agent

import (
	"errors"
	"testing"
	"time"
)

func envFrom(m map[string]string) func(string) string {
	return func(k string) string { return m[k] }
}

func noReadFile(string) ([]byte, error) { return nil, errors.New("no file") }

func TestLoadConfigDefaults(t *testing.T) {
	c, err := LoadConfig(envFrom(map[string]string{
		"GAMESTREAM_PROFILES": "brad=game-brad",
	}), noReadFile)
	if err != nil {
		t.Fatal(err)
	}
	if c.Broker != "192.168.3.6:1883" {
		t.Errorf("broker = %q", c.Broker)
	}
	if c.UnitTemplate != "gamestream@%s.service" {
		t.Errorf("unit template = %q", c.UnitTemplate)
	}
	if c.StartTimeout != 120*time.Second {
		t.Errorf("start timeout = %v", c.StartTimeout)
	}
	if len(c.Profiles) != 1 || c.Profiles[0].ID != "brad" || c.Profiles[0].Instance != "game-brad" {
		t.Errorf("profiles = %+v", c.Profiles)
	}
}

func TestLoadConfigOverrides(t *testing.T) {
	c, err := LoadConfig(envFrom(map[string]string{
		"GAMESTREAM_MQTT_BROKER":   "10.0.0.1:1883",
		"GAMESTREAM_MQTT_PASSWORD": "hunter2",
		"GAMESTREAM_TOPIC_PREFIX":  "romeo/gs/",
		"GAMESTREAM_START_TIMEOUT": "30s",
		"GAMESTREAM_PROFILES":      "brad=game-brad, hannah=game-hannah",
	}), noReadFile)
	if err != nil {
		t.Fatal(err)
	}
	if c.Broker != "10.0.0.1:1883" || c.Password != "hunter2" {
		t.Errorf("overrides not applied: %+v", c)
	}
	if c.TopicPrefix != "romeo/gs" {
		t.Errorf("trailing slash not trimmed: %q", c.TopicPrefix)
	}
	if c.StartTimeout != 30*time.Second {
		t.Errorf("timeout = %v", c.StartTimeout)
	}
	if len(c.Profiles) != 2 {
		t.Errorf("want 2 profiles, got %+v", c.Profiles)
	}
}

func TestLoadConfigPasswordFile(t *testing.T) {
	read := func(p string) ([]byte, error) {
		if p != "/secret" {
			return nil, errors.New("wrong path")
		}
		return []byte("filesecret\n"), nil
	}
	c, err := LoadConfig(envFrom(map[string]string{
		"GAMESTREAM_MQTT_PASSWORD_FILE": "/secret",
		"GAMESTREAM_PROFILES":           "brad=game-brad",
	}), read)
	if err != nil {
		t.Fatal(err)
	}
	if c.Password != "filesecret" {
		t.Errorf("password = %q, want trimmed filesecret", c.Password)
	}
}

func TestLoadConfigBadTimeout(t *testing.T) {
	_, err := LoadConfig(envFrom(map[string]string{
		"GAMESTREAM_START_TIMEOUT": "nope",
		"GAMESTREAM_PROFILES":      "brad=game-brad",
	}), noReadFile)
	if err == nil {
		t.Fatal("want error for bad timeout")
	}
}

func TestParseProfilesErrors(t *testing.T) {
	for _, bad := range []string{"brad", "=game-brad", "brad=", "a=b=c"} {
		if _, err := ParseProfiles(bad); err == nil {
			t.Errorf("ParseProfiles(%q) = nil error, want error", bad)
		}
	}
	if p, err := ParseProfiles(""); err != nil || p != nil {
		t.Errorf("empty profiles = %+v, %v", p, err)
	}
}

func TestValidate(t *testing.T) {
	base := func() Config {
		return Config{
			Broker:       "b:1883",
			UnitTemplate: "gamestream@%s.service",
			Profiles:     []Profile{{ID: "brad", Instance: "game-brad"}},
		}
	}
	if err := base().Validate(); err != nil {
		t.Fatalf("valid config rejected: %v", err)
	}

	c := base()
	c.Broker = ""
	if err := c.Validate(); err == nil {
		t.Error("empty broker should fail")
	}

	c = base()
	c.UnitTemplate = "gamestream.service"
	if err := c.Validate(); err == nil {
		t.Error("template without a placeholder should fail")
	}

	c = base()
	c.Profiles = nil
	if err := c.Validate(); err == nil {
		t.Error("no profiles should fail")
	}

	c = base()
	c.Profiles = []Profile{{ID: "brad", Instance: "a"}, {ID: "brad", Instance: "b"}}
	if err := c.Validate(); err == nil {
		t.Error("duplicate id should fail")
	}

	c = base()
	c.Profiles = []Profile{{ID: "a", Instance: "x"}, {ID: "b", Instance: "x"}}
	if err := c.Validate(); err == nil {
		t.Error("duplicate instance should fail")
	}
}

func TestUnitAndTopics(t *testing.T) {
	c := defaultConfig()
	c.Profiles = []Profile{{ID: "brad", Instance: "game-brad"}}
	p := c.Profiles[0]
	if got := c.Unit(p); got != "gamestream@game-brad.service" {
		t.Errorf("unit = %q", got)
	}
	if got := c.CommandTopic(p); got != "romeo/gamestream/brad/set" {
		t.Errorf("command topic = %q", got)
	}
	if got := c.SwitchStateTopic(p); got != "romeo/gamestream/brad/switch" {
		t.Errorf("switch topic = %q", got)
	}
	if got := c.SensorStateTopic(p); got != "romeo/gamestream/brad/state" {
		t.Errorf("sensor topic = %q", got)
	}
	if got := c.AvailabilityTopic(); got != "romeo/gamestream/availability" {
		t.Errorf("availability topic = %q", got)
	}
	if got := c.SwitchDiscoveryTopic(p); got != "homeassistant/switch/romeo_gamestream_brad/config" {
		t.Errorf("switch discovery topic = %q", got)
	}
	if got := c.SensorDiscoveryTopic(p); got != "homeassistant/sensor/romeo_gamestream_brad/config" {
		t.Errorf("sensor discovery topic = %q", got)
	}
}
