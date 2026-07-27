// Command gamestream-agent bridges Home Assistant (over MQTT) and on-demand
// game-streaming sessions (systemd units) on a headless host.
//
// Subcommands:
//
//	gamestream-agent serve
//	    Run the daemon: connect to MQTT, publish Home Assistant discovery, and
//	    start/stop the gamestream@<instance> units in response to switch
//	    commands while reporting live state back.
//
//	gamestream-agent notify --profile <id> --event stream-start|stream-stop
//	    Report a Sunshine stream start/stop to the running daemon (called from
//	    Sunshine's global_prep_cmd hook).
package main

import (
	"context"
	"flag"
	"fmt"
	"log/slog"
	"os"
	"os/signal"
	"syscall"
	"time"

	"gamestream-agent/agent"
)

func main() {
	if len(os.Args) < 2 {
		usage()
		os.Exit(2)
	}
	switch os.Args[1] {
	case "serve":
		if err := runServe(); err != nil {
			fmt.Fprintln(os.Stderr, "gamestream-agent serve:", err)
			os.Exit(1)
		}
	case "notify":
		if err := runNotify(os.Args[2:]); err != nil {
			fmt.Fprintln(os.Stderr, "gamestream-agent notify:", err)
			os.Exit(1)
		}
	case "-h", "--help", "help":
		usage()
	default:
		usage()
		os.Exit(2)
	}
}

func usage() {
	fmt.Fprintln(os.Stderr, "usage: gamestream-agent <serve|notify> [flags]")
}

func runServe() error {
	log := slog.New(slog.NewTextHandler(os.Stderr, nil))

	cfg, err := agent.LoadConfigFromEnv()
	if err != nil {
		return err
	}
	if err := cfg.Validate(); err != nil {
		return err
	}

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	sd, err := agent.NewSystemdController(ctx)
	if err != nil {
		return fmt.Errorf("connecting to systemd: %w", err)
	}
	defer sd.Close()

	rt := agent.NewRuntime(cfg, nil, sd, log)

	pub := agent.NewPahoPublisher(cfg, rt.OnConnect)
	rt.SetPublisher(pub)
	if err := pub.Connect(); err != nil {
		return fmt.Errorf("connecting to mqtt: %w", err)
	}
	defer pub.Disconnect()

	log.Info("gamestream-agent started",
		"broker", cfg.Broker, "profiles", cfg.ProfileIDs(), "socket", cfg.SocketPath)
	return rt.Run(ctx)
}

func runNotify(args []string) error {
	fs := flag.NewFlagSet("notify", flag.ContinueOnError)
	profile := fs.String("profile", "", "profile id (optional; empty targets the active session)")
	event := fs.String("event", "", "stream-start or stream-stop")
	socket := fs.String("socket", "", "agent notify socket path (defaults to config)")
	if err := fs.Parse(args); err != nil {
		return err
	}
	var streaming bool
	switch *event {
	case "stream-start":
		streaming = true
	case "stream-stop":
		streaming = false
	default:
		return fmt.Errorf("--event must be stream-start or stream-stop")
	}

	sockPath := *socket
	if sockPath == "" {
		cfg, err := agent.LoadConfigFromEnv()
		if err != nil {
			return err
		}
		sockPath = cfg.SocketPath
	}

	return agent.SendNotify(sockPath, agent.NotifyMsg{Profile: *profile, Streaming: streaming}, 5*time.Second)
}
