# Lower the internal panel's refresh rate on the power-saver profile, restore it
# otherwise. Watches tuned-ppd / power-profiles-daemon's ActiveProfile over the
# system D-Bus, so it reacts whether the profile is switched from KDE's battery
# applet or from the command line.

# kscreen-doctor and gdbus ship in the system profile on this host; jq is
# provided via the service's runtimeInputs.
export PATH="/run/current-system/sw/bin:/usr/bin:${PATH:-}"

OUTPUT="eDP-1"

get_profile() {
  gdbus call --system \
    --dest net.hadess.PowerProfiles \
    --object-path /net/hadess/PowerProfiles \
    --method org.freedesktop.DBus.Properties.Get \
    net.hadess.PowerProfiles ActiveProfile 2>/dev/null \
    | sed -E "s/^\(<'(.*)'>,\)$/\1/"
}

apply() {
  local profile pick json result cur target
  profile="$1"

  # power-saver -> lowest available rate; anything else -> highest.
  case "$profile" in
    power-saver) pick="min_by" ;;
    *) pick="max_by" ;;
  esac

  json="$(kscreen-doctor -j 2>/dev/null)" || return 0

  # At the output's current resolution, choose the target mode's refresh rate.
  # Emits "<currentModeId> <targetModeId>".
  result="$(printf '%s' "$json" | jq -r --arg o "$OUTPUT" "
    .outputs[] | select(.name==\$o and .connected==true and .enabled==true)
    | .currentModeId as \$cur
    | (.modes[] | select(.id==\$cur) | .size) as \$sz
    | ([ .modes[] | select(.size.width==\$sz.width and .size.height==\$sz.height) ] | ${pick}(.refreshRate)) as \$t
    | \"\(\$cur) \(\$t.id)\"
  ")" || return 0

  cur="${result%% *}"
  target="${result##* }"
  [ -n "$target" ] || return 0
  [ "$cur" = "$target" ] && return 0

  echo "power-saver-refresh: profile=$profile, setting $OUTPUT to mode $target"
  kscreen-doctor "output.$OUTPUT.mode.$target"
}

# Apply once for the current profile, then react to every change.
apply "$(get_profile)"

gdbus monitor --system --dest net.hadess.PowerProfiles | while read -r line; do
  case "$line" in
    *ActiveProfile*) apply "$(get_profile)" ;;
  esac
done
