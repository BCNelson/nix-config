{ config, lib, pkgs, ... }:

let
  cfg = config.services.bcnelson.sign;
  tabSwitcher = pkgs.writeShellApplication {
    name = "tab-switcher";
    runtimeInputs = [ pkgs.ydotool ];
    text = ''
      #!/usr/bin/env bash
      set -euo pipefail
      echo "Starting tab switcher with a initial delay of ${builtins.toString cfg.startDelay} seconds and a sleep interval of ${builtins.toString cfg.sleepInterval} seconds"
      sleep ${builtins.toString cfg.startDelay} # Wait for Cage to start
      ydotool key 87:1 87:0 # F11
      while true; do
        sleep ${builtins.toString cfg.sleepInterval}
        if compgen -G "/dev/input/by-path/*-kbd"; then
          echo "Keyboard detected, skipping tab switch"
          continue
        fi
        ydotool key 29:1 15:1 15:0 29:0 # Ctrl+Tab
      done
    '';
  };
in
{
  options = {
    services.bcnelson.sign = {
      enable = lib.mkEnableOption "Enable the Sign service";
      urls = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        description = ''
          List of URLs to display on the screen
        '';
      };
      sleepInterval = lib.mkOption {
        type = lib.types.int;
        default = 15;
        description = ''
          Interval in seconds to switch tabs
        '';
      };
      startDelay = lib.mkOption {
        type = lib.types.int;
        default = 20;
        description = ''
          Delay in seconds before starting to switch tabs
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.sign = {
      isNormalUser = true;
      home = "/var/lib/sign";
      extraGroups = [ "ydotool" ];
    };

    services.cage = {
      enable = true;
      user = "sign";
      extraArguments = [ "-d" ];
      program = "${pkgs.firefox}/bin/firefox ${lib.concatStringsSep " " cfg.urls}";
    };

    systemd.services."cage-tty1".after = [ "network-online.target" ];

    programs.firefox = {
      enable = true;
      preferences = {
        "trailhead.firstrun.didSeeAboutWelcome" = true;
        "browser.startup.homepage" = "chrome://browser/content/blanktab.html";
        "datareporting.policy.dataSubmissionPolicyBypassNotification" = true;
        "browser.tabs.unloadOnLowMemory" = false;
        "browser.sessionstore.resume_from_crash" = false;
      };
    };

    programs.ydotool.enable = true;

    systemd.services.tab-switcher = {
      description = "Switch tabs every ${builtins.toString cfg.sleepInterval} seconds";
      wantedBy = [ "multi-user.target" ];
      partOf = [ "cage-tty1.service" ];
      serviceConfig = {
        Type = "simple";
        ExecStart = "${tabSwitcher}/bin/tab-switcher";
        Restart = "always";
      };
      environment = {
        YDOTOOL_SOCKET = "${config.environment.variables.YDOTOOL_SOCKET}";
      };
    };
  };
}