{ config, pkgs, ... }:
let
  ntfyTopicFile = "/run/agenix/happy_ntfy_topic";
  happyHomeDir = "${config.xdg.dataHome}/happy";

  happy-coder = pkgs.symlinkJoin {
    name = "happy-coder-wrapped-${pkgs.happy-coder.version}";
    paths = [ pkgs.happy-coder ];
    nativeBuildInputs = [ pkgs.makeWrapper ];
    postBuild = ''
      wrapProgram $out/bin/happy \
        --set-default HAPPY_HOME_DIR ${happyHomeDir}
      wrapProgram $out/bin/happy-mcp \
        --set-default HAPPY_HOME_DIR ${happyHomeDir}
    '';
    inherit (pkgs.happy-coder) meta;
  };

  happyAuthNotify = pkgs.writeShellApplication {
    name = "happy-auth-notify";
    runtimeInputs = with pkgs; [
      happy-coder
      expect
      curl
      coreutils
    ];
    text = ''
      if [[ -f "${happyHomeDir}/access.key" ]]; then
        echo "happy-auth-notify: already authenticated, nothing to do"
        exit 0
      fi

      if [[ -z "''${NTFY_TOPIC_FILE:-}" || ! -r "''${NTFY_TOPIC_FILE}" ]]; then
        echo "happy-auth-notify: NTFY_TOPIC_FILE must point to a readable file" >&2
        exit 1
      fi

      TOPIC=$(tr -d '[:space:]' < "''${NTFY_TOPIC_FILE}")
      export TOPIC

      exec expect <<'EOF'
      set timeout -1
      log_user 1
      spawn happy auth login
      expect -re {How would you like to authenticate}
      send "1"
      set notified 0
      expect {
        -re {(happy://terminal\?[A-Za-z0-9_-]+)} {
          if {!$notified} {
            set url $expect_out(1,string)
            catch {
              exec curl -sS \
                -H "Title: Happy auth pairing" \
                -H "Click: $url" \
                -d "Tap to authorize this workstation as a happy machine" \
                "https://ntfy.sh/$env(TOPIC)"
            }
            set notified 1
          }
          exp_continue
        }
        eof {
          catch wait result
          exit [lindex $result 3]
        }
      }
      EOF
    '';
  };
in
{
  home.packages = [
    happy-coder
    happyAuthNotify
  ];

  systemd.user.services.happy-auth-bootstrap = {
    Unit = {
      Description = "Bootstrap happy authentication via ntfy notification";
      After = [ "network-online.target" ];
      Wants = [ "network-online.target" ];
      Before = [ "happy-daemon.service" ];
      ConditionPathExists = "!${happyHomeDir}/access.key";
    };

    Service = {
      Type = "oneshot";
      RemainAfterExit = true;
      Environment = [
        "PATH=${config.home.profileDirectory}/bin"
        "NTFY_TOPIC_FILE=${ntfyTopicFile}"
      ];
      ExecStart = "${happyAuthNotify}/bin/happy-auth-notify";
      TimeoutStartSec = "1h";
    };

    Install = {
      WantedBy = [ "default.target" ];
    };
  };

  systemd.user.services.happy-daemon = {
    Unit = {
      Description = "Happy remote agent daemon";
      After = [ "network-online.target" "happy-auth-bootstrap.service" ];
      Wants = [ "network-online.target" "happy-auth-bootstrap.service" ];
    };

    Service = {
      Environment = [ "PATH=${config.home.profileDirectory}/bin" ];
      ExecStart = "${happy-coder}/bin/happy daemon start-sync";
      Restart = "on-failure";
      RestartSec = 5;
    };

    Install = {
      WantedBy = [ "default.target" ];
    };
  };
}
