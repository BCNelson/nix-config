package agent

import (
	"fmt"
	"time"

	mqtt "github.com/eclipse/paho.mqtt.golang"
)

// PahoPublisher adapts the eclipse paho client to the Publisher interface.
type PahoPublisher struct {
	client mqtt.Client
}

// NewPahoPublisher builds the client. onConnect is invoked after every
// successful (re)connection so the runtime can (re)publish discovery and state.
// Call Connect to actually connect (after wiring the publisher into the runtime,
// since onConnect fires during connect).
func NewPahoPublisher(cfg Config, onConnect func()) *PahoPublisher {
	opts := mqtt.NewClientOptions()
	opts.AddBroker("tcp://" + cfg.Broker)
	opts.SetClientID(cfg.ClientID)
	if cfg.Username != "" {
		opts.SetUsername(cfg.Username)
	}
	if cfg.Password != "" {
		opts.SetPassword(cfg.Password)
	}
	opts.SetAutoReconnect(true)
	opts.SetConnectRetry(true)
	opts.SetConnectRetryInterval(5 * time.Second)
	opts.SetCleanSession(true)
	// Last will: mark the device unavailable if the agent drops off the broker.
	opts.SetBinaryWill(cfg.AvailabilityTopic(), []byte("offline"), 1, true)
	if onConnect != nil {
		opts.SetOnConnectHandler(func(mqtt.Client) { onConnect() })
	}
	return &PahoPublisher{client: mqtt.NewClient(opts)}
}

// Connect establishes the broker connection, triggering the onConnect handler.
func (p *PahoPublisher) Connect() error {
	token := p.client.Connect()
	if !token.WaitTimeout(30 * time.Second) {
		return fmt.Errorf("mqtt connect timed out")
	}
	return token.Error()
}

// Publish sends a message at QoS 1.
func (p *PahoPublisher) Publish(topic string, retained bool, payload []byte) error {
	token := p.client.Publish(topic, 1, retained, payload)
	token.Wait()
	return token.Error()
}

// Subscribe registers a handler for a topic at QoS 1.
func (p *PahoPublisher) Subscribe(topic string, handler func(payload []byte)) error {
	token := p.client.Subscribe(topic, 1, func(_ mqtt.Client, m mqtt.Message) {
		handler(m.Payload())
	})
	token.Wait()
	return token.Error()
}

// Disconnect cleanly disconnects from the broker.
func (p *PahoPublisher) Disconnect() {
	p.client.Disconnect(250)
}
