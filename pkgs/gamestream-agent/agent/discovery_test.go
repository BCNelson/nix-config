package agent

import (
	"encoding/json"
	"testing"
)

func discoveryConfig() Config {
	c := defaultConfig()
	c.Profiles = []Profile{
		{ID: "brad", Instance: "game-brad"},
		{ID: "hannah", Instance: "game-hannah"},
	}
	return c
}

func TestDiscoveryMessagesCount(t *testing.T) {
	c := discoveryConfig()
	msgs, err := c.DiscoveryMessages()
	if err != nil {
		t.Fatal(err)
	}
	// One switch + one sensor per profile.
	if len(msgs) != 4 {
		t.Fatalf("got %d messages, want 4", len(msgs))
	}
}

func TestSwitchDiscoveryPayload(t *testing.T) {
	c := discoveryConfig()
	msgs, err := c.DiscoveryMessages()
	if err != nil {
		t.Fatal(err)
	}
	// First message is brad's switch.
	if msgs[0].Topic != "homeassistant/switch/romeo_gamestream_brad/config" {
		t.Fatalf("switch topic = %q", msgs[0].Topic)
	}
	var m map[string]any
	if err := json.Unmarshal(msgs[0].Payload, &m); err != nil {
		t.Fatal(err)
	}
	checks := map[string]string{
		"unique_id":     "romeo_gamestream_brad_switch",
		"command_topic": "romeo/gamestream/brad/set",
		"state_topic":   "romeo/gamestream/brad/switch",
		"payload_on":    "ON",
		"payload_off":   "OFF",
		"state_on":      "ON",
		"state_off":     "OFF",
	}
	for k, want := range checks {
		if got, _ := m[k].(string); got != want {
			t.Errorf("switch[%q] = %q, want %q", k, got, want)
		}
	}
	dev, ok := m["device"].(map[string]any)
	if !ok {
		t.Fatal("device block missing")
	}
	ids, _ := dev["identifiers"].([]any)
	if len(ids) != 1 || ids[0] != "romeo_gamestream" {
		t.Errorf("device identifiers = %v", dev["identifiers"])
	}
}

func TestSensorDiscoveryPayload(t *testing.T) {
	c := discoveryConfig()
	msgs, err := c.DiscoveryMessages()
	if err != nil {
		t.Fatal(err)
	}
	// Second message is brad's sensor.
	if msgs[1].Topic != "homeassistant/sensor/romeo_gamestream_brad/config" {
		t.Fatalf("sensor topic = %q", msgs[1].Topic)
	}
	var m map[string]any
	if err := json.Unmarshal(msgs[1].Payload, &m); err != nil {
		t.Fatal(err)
	}
	if m["unique_id"] != "romeo_gamestream_brad_state" {
		t.Errorf("sensor unique_id = %v", m["unique_id"])
	}
	if m["state_topic"] != "romeo/gamestream/brad/state" {
		t.Errorf("sensor state_topic = %v", m["state_topic"])
	}
	if m["availability_topic"] != "romeo/gamestream/availability" {
		t.Errorf("sensor availability_topic = %v", m["availability_topic"])
	}
}
