package agent

import (
	"context"
	"time"

	"github.com/coreos/go-systemd/v22/dbus"
)

// SystemdController adapts the go-systemd D-Bus client to the UnitController
// interface. It talks to the system bus, so the agent needs (scoped) polkit
// permission to manage the gamestream@ units.
type SystemdController struct {
	conn *dbus.Conn
}

// NewSystemdController opens a connection to the systemd system bus.
func NewSystemdController(ctx context.Context) (*SystemdController, error) {
	conn, err := dbus.NewSystemdConnectionContext(ctx)
	if err != nil {
		return nil, err
	}
	return &SystemdController{conn: conn}, nil
}

// StartUnit starts a unit with job mode "replace".
func (s *SystemdController) StartUnit(name string) error {
	_, err := s.conn.StartUnitContext(context.Background(), name, "replace", nil)
	return err
}

// StopUnit stops a unit with job mode "replace".
func (s *SystemdController) StopUnit(name string) error {
	_, err := s.conn.StopUnitContext(context.Background(), name, "replace", nil)
	return err
}

// Watch polls the named units and emits their ActiveState whenever it changes.
func (s *SystemdController) Watch(ctx context.Context, units []string, ch chan<- UnitStatus) error {
	want := make(map[string]bool, len(units))
	for _, u := range units {
		want[u] = true
	}
	updates, errs := s.conn.SubscribeUnits(time.Second)
	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case err := <-errs:
			if err != nil {
				return err
			}
		case changed := <-updates:
			for name, status := range changed {
				if !want[name] {
					continue
				}
				state := ActiveState(UnitInactive)
				if status != nil {
					state = ActiveState(status.ActiveState)
				}
				select {
				case ch <- UnitStatus{Unit: name, ActiveState: state}:
				case <-ctx.Done():
					return ctx.Err()
				}
			}
		}
	}
}

// Close closes the D-Bus connection.
func (s *SystemdController) Close() { s.conn.Close() }
