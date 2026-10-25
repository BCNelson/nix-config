package jobs

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestLogsOnlyOnFailure(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	mk := func(id, exit string) {
		d := dir(id)
		if err := os.MkdirAll(d, 0o700); err != nil {
			t.Fatal(err)
		}
		_ = os.WriteFile(filepath.Join(d, "job.json"), []byte(`{"job_id":"`+id+`","kind":"build","started":"`+time.Now().Format(time.RFC3339)+`"}`), 0o600)
		_ = os.WriteFile(filepath.Join(d, "log"), []byte("building foo\nbuilding bar\nsomething broke\n"), 0o600)
		_ = os.WriteFile(filepath.Join(d, "exit"), []byte(exit), 0o600)
	}
	mk("j-ok", "0")
	mk("j-bad", "1")

	ok, err := Get("j-ok")
	if err != nil {
		t.Fatal(err)
	}
	if ok.Tail != "" || ok.Error != "" {
		t.Errorf("a successful job carries logs: tail=%q error=%q", ok.Tail, ok.Error)
	}
	bad, err := Get("j-bad")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(bad.Tail+bad.Error, "something broke") {
		t.Errorf("a failed job lost its log: tail=%q error=%q", bad.Tail, bad.Error)
	}
}
