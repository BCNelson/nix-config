package agent

import (
	"net"
	"path/filepath"
	"testing"
	"time"
)

func TestNotifyCodecRoundTrip(t *testing.T) {
	for _, m := range []NotifyMsg{
		{Profile: "brad", Streaming: true},
		{Profile: "hannah", Streaming: false},
	} {
		b, err := EncodeNotify(m)
		if err != nil {
			t.Fatal(err)
		}
		if b[len(b)-1] != '\n' {
			t.Fatalf("encoded message must be newline-terminated")
		}
		got, err := DecodeNotify(b)
		if err != nil {
			t.Fatal(err)
		}
		if got != m {
			t.Fatalf("round trip = %+v, want %+v", got, m)
		}
	}
}

func TestNotifyEmptyProfileAllowed(t *testing.T) {
	// An empty profile is valid and means "the active session".
	b, err := EncodeNotify(NotifyMsg{Streaming: true})
	if err != nil {
		t.Fatal(err)
	}
	m, err := DecodeNotify(b)
	if err != nil {
		t.Fatal(err)
	}
	if m.Profile != "" || !m.Streaming {
		t.Fatalf("round trip = %+v", m)
	}
}

func TestDecodeNotifyRejectsBad(t *testing.T) {
	if _, err := DecodeNotify([]byte("{not json")); err == nil {
		t.Fatal("want error for bad json")
	}
}

func TestServeAndSendNotify(t *testing.T) {
	sock := filepath.Join(t.TempDir(), "notify.sock")
	l, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()

	got := make(chan NotifyMsg, 1)
	go func() { _ = ServeNotify(l, func(m NotifyMsg) { got <- m }) }()

	if err := SendNotify(sock, NotifyMsg{Profile: "brad", Streaming: true}, 2*time.Second); err != nil {
		t.Fatal(err)
	}
	select {
	case m := <-got:
		if m.Profile != "brad" || !m.Streaming {
			t.Fatalf("handler got %+v", m)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("handler was not invoked")
	}
}
