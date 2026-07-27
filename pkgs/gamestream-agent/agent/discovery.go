package agent

import "encoding/json"

// haDevice is the shared Home Assistant device block that groups all the
// agent's entities under one device.
type haDevice struct {
	Identifiers  []string `json:"identifiers"`
	Name         string   `json:"name"`
	Manufacturer string   `json:"manufacturer,omitempty"`
	Model        string   `json:"model,omitempty"`
}

type switchDiscovery struct {
	Name              string   `json:"name"`
	UniqueID          string   `json:"unique_id"`
	CommandTopic      string   `json:"command_topic"`
	StateTopic        string   `json:"state_topic"`
	PayloadOn         string   `json:"payload_on"`
	PayloadOff        string   `json:"payload_off"`
	StateOn           string   `json:"state_on"`
	StateOff          string   `json:"state_off"`
	AvailabilityTopic string   `json:"availability_topic"`
	Icon              string   `json:"icon,omitempty"`
	Device            haDevice `json:"device"`
}

type sensorDiscovery struct {
	Name              string   `json:"name"`
	UniqueID          string   `json:"unique_id"`
	StateTopic        string   `json:"state_topic"`
	AvailabilityTopic string   `json:"availability_topic"`
	Icon              string   `json:"icon,omitempty"`
	Device            haDevice `json:"device"`
}

// DiscoveryMessage is a single retained MQTT Discovery publish.
type DiscoveryMessage struct {
	Topic   string
	Payload []byte
}

func (c Config) device() haDevice {
	return haDevice{
		Identifiers:  []string{c.NodeID},
		Name:         c.DeviceName,
		Manufacturer: "Sunshine",
		Model:        "gamestream-agent",
	}
}

// DiscoveryMessages returns the retained Home Assistant MQTT Discovery configs
// for every profile: one switch (control) and one sensor (detailed state).
func (c Config) DiscoveryMessages() ([]DiscoveryMessage, error) {
	var msgs []DiscoveryMessage
	dev := c.device()
	for _, p := range c.Profiles {
		sw := switchDiscovery{
			Name:              p.ID,
			UniqueID:          c.NodeID + "_" + p.ID + "_switch",
			CommandTopic:      c.CommandTopic(p),
			StateTopic:        c.SwitchStateTopic(p),
			PayloadOn:         "ON",
			PayloadOff:        "OFF",
			StateOn:           "ON",
			StateOff:          "OFF",
			AvailabilityTopic: c.AvailabilityTopic(),
			Icon:              "mdi:controller",
			Device:            dev,
		}
		swb, err := json.Marshal(sw)
		if err != nil {
			return nil, err
		}
		msgs = append(msgs, DiscoveryMessage{Topic: c.SwitchDiscoveryTopic(p), Payload: swb})

		se := sensorDiscovery{
			Name:              p.ID + " state",
			UniqueID:          c.NodeID + "_" + p.ID + "_state",
			StateTopic:        c.SensorStateTopic(p),
			AvailabilityTopic: c.AvailabilityTopic(),
			Icon:              "mdi:monitor-dashboard",
			Device:            dev,
		}
		seb, err := json.Marshal(se)
		if err != nil {
			return nil, err
		}
		msgs = append(msgs, DiscoveryMessage{Topic: c.SensorDiscoveryTopic(p), Payload: seb})
	}
	return msgs, nil
}
