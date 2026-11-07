// hc-tailscale lets herdr-core (`hc`) reach machines over a tailnet the host's
// own tailscaled is not on. A user daemon keeps one embedded tsnet node per
// "slot" (one slot per tailnet login); `hc-tailscale ssh` is the --via command
// hc runs, and talks Tailscale SSH through the daemon's node:
//
//	hc-tailscale login
//	hc machine add --via 'hc-tailscale ssh bcnelson@romeo' --name romeo
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"log"
	"os"
	"os/signal"
	"syscall"
)

const usage = `hc-tailscale: Tailscale SSH transport for herdr-core (hc machine add --via)

Usage:
  hc-tailscale serve                                   run the daemon (systemd user service)
  hc-tailscale [--slot S] login [--hostname H]         sign slot S into a tailnet
  hc-tailscale [--slot S] status                       show slot S (slots: every slot)
  hc-tailscale slots                                   show every slot
  hc-tailscale [--slot S] logout                       sign slot S out
  hc-tailscale [--slot S] ssh [user@]host[:port] CMD   run CMD there; the --via entry point

A slot is one tsnet node with its own login, so it can join its own tailnet;
the default slot is "default". One slot reaches every machine on its tailnet:

  hc machine add --via 'hc-tailscale ssh bcnelson@romeo' --name romeo
  hc machine add --via 'hc-tailscale --slot work ssh me@devbox' --name devbox
`

func main() {
	os.Exit(run(os.Args[1:]))
}

func run(argv []string) int {
	fs := flag.NewFlagSet("hc-tailscale", flag.ContinueOnError)
	fs.Usage = func() { fmt.Fprint(os.Stderr, usage) }
	slot := fs.String("slot", envOr("HC_TAILSCALE_SLOT", defaultSlot), "slot (tsnet node) to use")
	if err := fs.Parse(argv); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return 0
		}
		return 2
	}
	args := fs.Args()
	if len(args) == 0 {
		fmt.Fprint(os.Stderr, usage)
		return 2
	}
	if err := validSlot(*slot); err != nil {
		fmt.Fprintln(os.Stderr, "hc-tailscale:", err)
		return 2
	}

	var err error
	switch args[0] {
	case "serve":
		err = serve()
	case "login":
		lf := flag.NewFlagSet("login", flag.ContinueOnError)
		hostname := lf.String("hostname", "", "device name for a new slot (default <machine>-hc[-<slot>])")
		if lf.Parse(args[1:]) != nil {
			return 2
		}
		err = runLogin(*slot, *hostname, os.Stdout)
	case "status":
		err = runSimple("status", *slot, os.Stdout)
	case "slots":
		err = runSimple("status", "", os.Stdout)
	case "logout":
		err = runSimple("logout", *slot, os.Stdout)
	case "ssh":
		if len(args) < 2 {
			fmt.Fprint(os.Stderr, usage)
			return 2
		}
		err = runSSH(*slot, args[1], args[2:])
	case "help", "-h", "--help":
		fmt.Fprint(os.Stdout, usage)
		return 0
	default:
		fmt.Fprintf(os.Stderr, "hc-tailscale: unknown command %q\n\n%s", args[0], usage)
		return 2
	}

	if err == nil {
		return 0
	}
	var ee *exitError
	if errors.As(err, &ee) {
		if ee.msg != "" {
			fmt.Fprintln(os.Stderr, "hc-tailscale:", ee.msg)
		}
		return ee.code
	}
	fmt.Fprintln(os.Stderr, "hc-tailscale:", err)
	return 1
}

func envOr(k, def string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return def
}

func serve() error {
	base, err := stateBase()
	if err != nil {
		return err
	}
	if err := os.MkdirAll(base, 0o700); err != nil {
		return err
	}
	path, err := socketPath()
	if err != nil {
		return err
	}
	ln, err := listen(path)
	if err != nil {
		return err
	}
	defer os.Remove(path)

	logger := log.New(os.Stderr, "", 0)
	d := newDaemon(base, logger)
	defer d.closeAll()
	d.startKnown()

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()
	logger.Printf("listening on %s, state in %s", path, base)
	return d.serve(ctx, ln)
}
