package agent

import (
	"bufio"
	"encoding/json"
	"fmt"
	"net"
	"time"
)

// NotifyMsg is the message the Sunshine global_prep_cmd hook sends to the
// running agent over the unix socket to report a stream start/stop.
//
// Profile may be empty, in which case the daemon routes the event to whichever
// profile currently owns the session ("the active one"). This lets a single,
// shared Sunshine config use one identical hook for every profile.
type NotifyMsg struct {
	Profile   string `json:"profile,omitempty"`
	Streaming bool   `json:"streaming"`
}

// EncodeNotify serializes a NotifyMsg as a single newline-terminated JSON line.
func EncodeNotify(m NotifyMsg) ([]byte, error) {
	b, err := json.Marshal(m)
	if err != nil {
		return nil, err
	}
	return append(b, '\n'), nil
}

// DecodeNotify parses a NotifyMsg from a JSON line.
func DecodeNotify(b []byte) (NotifyMsg, error) {
	var m NotifyMsg
	if err := json.Unmarshal(b, &m); err != nil {
		return NotifyMsg{}, err
	}
	return m, nil
}

// SendNotify connects to the agent's unix socket, sends a NotifyMsg and waits
// for the "ok" acknowledgement.
func SendNotify(socketPath string, m NotifyMsg, timeout time.Duration) error {
	payload, err := EncodeNotify(m)
	if err != nil {
		return err
	}
	conn, err := net.DialTimeout("unix", socketPath, timeout)
	if err != nil {
		return err
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(timeout))
	if _, err := conn.Write(payload); err != nil {
		return err
	}
	resp, err := bufio.NewReader(conn).ReadString('\n')
	if err != nil {
		return err
	}
	if resp != "ok\n" {
		return fmt.Errorf("notify: unexpected response %q", resp)
	}
	return nil
}

// ServeNotify accepts connections on l and invokes handler for each valid
// NotifyMsg. It returns when l is closed.
func ServeNotify(l net.Listener, handler func(NotifyMsg)) error {
	for {
		conn, err := l.Accept()
		if err != nil {
			return err
		}
		go handleNotifyConn(conn, handler)
	}
}

func handleNotifyConn(conn net.Conn, handler func(NotifyMsg)) {
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
	line, err := bufio.NewReader(conn).ReadBytes('\n')
	if err != nil {
		return
	}
	m, err := DecodeNotify(line)
	if err != nil {
		_, _ = conn.Write([]byte("error\n"))
		return
	}
	handler(m)
	_, _ = conn.Write([]byte("ok\n"))
}
