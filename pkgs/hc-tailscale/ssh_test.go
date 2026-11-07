package main

import (
	"bytes"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/binary"
	"io"
	"net"
	"os"
	"testing"

	"golang.org/x/crypto/ssh"
)

// fakeSSHServer accepts "none" auth like Tailscale SSH. For an exec request it
// writes "cmd=<command>\n", then echoes stdin back until EOF, then exits with
// status 7. With banner set it instead refuses auth after sending it.
func fakeSSHServer(t *testing.T, banner string) string {
	t.Helper()
	_, priv, _ := ed25519.GenerateKey(rand.Reader)
	signer, err := ssh.NewSignerFromKey(priv)
	if err != nil {
		t.Fatal(err)
	}
	cfg := &ssh.ServerConfig{
		NoClientAuth:   banner == "",
		BannerCallback: func(ssh.ConnMetadata) string { return banner },
		KeyboardInteractiveCallback: func(ssh.ConnMetadata, ssh.KeyboardInteractiveChallenge) (*ssh.Permissions, error) {
			select {} // check mode: hangs until a human acts
		},
	}
	cfg.AddHostKey(signer)

	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			nc, err := ln.Accept()
			if err != nil {
				return
			}
			go func() {
				_, chans, reqs, err := ssh.NewServerConn(nc, cfg)
				if err != nil {
					nc.Close()
					return
				}
				go ssh.DiscardRequests(reqs)
				for nch := range chans {
					ch, creqs, _ := nch.Accept()
					go func() {
						for r := range creqs {
							if r.Type != "exec" {
								r.Reply(false, nil)
								continue
							}
							r.Reply(true, nil)
							n := binary.BigEndian.Uint32(r.Payload)
							io.WriteString(ch, "cmd="+string(r.Payload[4:4+n])+"\n")
							io.Copy(ch, ch)
							ch.SendRequest("exit-status", false, ssh.Marshal(struct{ S uint32 }{7}))
							ch.Close()
						}
					}()
				}
			}()
		}
	}()
	return ln.Addr().String()
}

// withStdio swaps os.Stdin/os.Stdout for pipes; finish restores them and
// returns everything written to stdout.
func withStdio(t *testing.T, in []byte) (finish func() string) {
	t.Helper()
	ir, iw, _ := os.Pipe()
	or, ow, _ := os.Pipe()
	oldIn, oldOut := os.Stdin, os.Stdout
	os.Stdin, os.Stdout = ir, ow
	go func() { iw.Write(in); iw.Close() }()
	out := &bytes.Buffer{}
	done := make(chan struct{})
	go func() { io.Copy(out, or); close(done) }()
	var once bool
	finish = func() string {
		if !once {
			once = true
			os.Stdin, os.Stdout = oldIn, oldOut
			ow.Close()
			<-done
		}
		return out.String()
	}
	t.Cleanup(func() { finish() })
	return finish
}

func loggedInDaemon(t *testing.T, target string) {
	startDaemon(t, func(slot, _, host string) (node, error) {
		return &fakeNode{slot: slot, hostname: host, target: target}, nil
	})
	if err := runLogin(defaultSlot, "", io.Discard); err != nil {
		t.Fatal(err)
	}
}

func TestSSHRunPassesBytesAndExitCode(t *testing.T) {
	loggedInDaemon(t, fakeSSHServer(t, ""))
	in := []byte("\x00raw\xffbytes\n")
	finish := withStdio(t, in)
	err := runSSH(defaultSlot, "bob@romeo", []string{`exec hc connect 'a b'`})
	got := finish()
	ee, ok := err.(*exitError)
	if !ok || ee.code != 7 || ee.msg != "" {
		t.Fatalf("runSSH = %#v, want exit 7", err)
	}
	want := "cmd=exec hc connect 'a b'\n" + string(in)
	if got != want {
		t.Errorf("stdout = %q, want %q", got, want)
	}
}

func TestSSHCheckModeFailsFast(t *testing.T) {
	loggedInDaemon(t, fakeSSHServer(t, "# Tailscale SSH requires an additional check.\n# To authenticate, visit: https://login.tailscale.com/a/xyz\n"))
	withStdio(t, nil)
	err := runSSH(defaultSlot, "bob@whiskey", []string{"true"})
	ee, ok := err.(*exitError)
	if !ok || ee.code != exitTransport || !bytes.Contains([]byte(ee.msg), []byte("https://login.tailscale.com/a/xyz")) {
		t.Fatalf("runSSH = %#v", err)
	}
}
