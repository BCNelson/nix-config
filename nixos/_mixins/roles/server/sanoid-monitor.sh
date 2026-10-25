# Usage: sanoid-monitor snapshots|capacity
#
# Runs sanoid's Nagios-style --monitor-<mode> and turns each finding into a
# Prometheus series in $CHECK_METRICS, so every dataset or pool alerts on its
# own with the numbers in hand. sanoid only reports text such as
#
#   WARN: vault/Backups/NelsonData newest hourly snapshot is 2h 3m 29s old (should be < 1h 30m 0s)
#   CRIT: vault/remotebackups/VorLevel2 has no daily snapshots at all!
#   *** WARNING *** OK ZPOOL liveData : 52%, WARNING ZPOOL vault : 84%
#
# Exits non-zero only when sanoid could not judge (UNKNOWN) or a finding did
# not parse, so a format change shows up as a broken check instead of
# findings silently going missing.

mode=$1

rc=0
out=$(sanoid --configdir /etc/sanoid "--monitor-$mode" 2>&1) || rc=$?
# Drop the per-template config noise; keep the rest for the journal.
out=$(grep -v '^ignored key' <<<"$out" || true)
echo "$out"

# sanoid's displaytime: "1d 2h 3m 4s" -> seconds
to_seconds() {
  local total=0 part
  for part in $1; do
    case $part in
      *d) total=$((total + ${part%d} * 86400)) ;;
      *h) total=$((total + ${part%h} * 3600)) ;;
      *m) total=$((total + ${part%m} * 60)) ;;
      *s) total=$((total + ${part%s})) ;;
    esac
  done
  echo "$total"
}

level() {
  case $1 in
    CRIT | CRITICAL) echo critical ;;
    WARN | WARNING) echo warning ;;
    *) echo ok ;;
  esac
}

unparsed=0
case $mode in
  snapshots)
    snap_re='^(WARN|CRIT): (.+) newest ([a-z]+) snapshot is (.+) old \(should be < (.+)\)$'
    missing_re='^(WARN|CRIT): (.+) has no ([a-z]+) snapshots at all!$'
    {
      echo "# HELP sanoid_snapshot_age_seconds Age of the newest snapshot of a type, for datasets past their sanoid limit."
      echo "# TYPE sanoid_snapshot_age_seconds gauge"
      echo "# HELP sanoid_snapshot_missing A dataset has no snapshots of a type its sanoid policy keeps."
      echo "# TYPE sanoid_snapshot_missing gauge"
    } >>"$CHECK_METRICS"
    # Findings are joined with ", "; dataset names have no commas.
    while IFS= read -r msg; do
      if [[ $msg =~ $snap_re ]]; then
        printf 'sanoid_snapshot_age_seconds{dataset="%s",type="%s",level="%s",limit="%s"} %s\n' \
          "${BASH_REMATCH[2]}" "${BASH_REMATCH[3]}" "$(level "${BASH_REMATCH[1]}")" "${BASH_REMATCH[5]}" \
          "$(to_seconds "${BASH_REMATCH[4]}")" >>"$CHECK_METRICS"
      elif [[ $msg =~ $missing_re ]]; then
        printf 'sanoid_snapshot_missing{dataset="%s",type="%s",level="%s"} 1\n' \
          "${BASH_REMATCH[2]}" "${BASH_REMATCH[3]}" "$(level "${BASH_REMATCH[1]}")" >>"$CHECK_METRICS"
      else
        echo "could not parse: $msg" >&2
        unparsed=1
      fi
    done < <(grep -oE '(WARN|CRIT): [^,]+' <<<"$out" || true)

    # Transfers in flight, so a snapshot alert can say a dataset is behind
    # because it is still receiving. A receive always runs on the host of the
    # dataset it fills (syncoid runs it over ssh when pushing), and its resume
    # token names the snapshot and how many bytes have landed. The matching
    # send runs on the source host, where `zfs send -nP` estimates the total;
    # the alert joins the two on the snapshot name.
    {
      echo "# HELP zfs_receive_running_seconds How long a zfs receive into a dataset has been running."
      echo "# TYPE zfs_receive_running_seconds gauge"
      echo "# HELP zfs_receive_bytes Bytes a resumable zfs receive has written so far, from its resume token."
      echo "# TYPE zfs_receive_bytes gauge"
      echo "# HELP zfs_send_running_seconds How long a zfs send of a snapshot has been running."
      echo "# TYPE zfs_send_running_seconds gauge"
      echo "# HELP zfs_send_estimated_bytes zfs send -nP's estimate of a running send's total size."
      echo "# TYPE zfs_send_estimated_bytes gauge"
    } >>"$CHECK_METRICS"
    for pid in $(pgrep -x zfs || true); do
      # cmdline rather than ps: dataset names survive intact, without quoting.
      mapfile -d '' -t argv <"/proc/$pid/cmdline" 2>/dev/null || continue
      [ "${#argv[@]}" -ge 3 ] || continue
      elapsed=$(ps -o etimes= -p "$pid" | tr -d ' ' || true)
      [ -n "$elapsed" ] || continue
      case ${argv[1]} in
        receive | recv)
          dataset=${argv[-1]}
          snapshot=
          bytes=
          token=$(zfs get -H -o value receive_resume_token "$dataset" 2>/dev/null || true)
          if [ -n "$token" ] && [ "$token" != "-" ]; then
            # Exits non-zero here (the snapshot is on the sender), but
            # prints the token's contents first.
            contents=$(zfs send -nvt "$token" 2>&1 || true)
            snapshot=$(sed -n 's/^[[:space:]]*toname = //p' <<<"$contents")
            hex=$(sed -n 's/^[[:space:]]*bytes = 0x//p' <<<"$contents")
            if [ -n "$hex" ]; then bytes=$((16#$hex)); fi
          fi
          printf 'zfs_receive_running_seconds{dataset="%s",snapshot="%s"} %s\n' "$dataset" "$snapshot" "$elapsed" >>"$CHECK_METRICS"
          if [ -n "$bytes" ]; then
            printf 'zfs_receive_bytes{dataset="%s",snapshot="%s"} %s\n' "$dataset" "$snapshot" "$bytes" >>"$CHECK_METRICS"
          fi
          ;;
        send)
          # Ignore our own dry runs and token decodes.
          case " ${argv[*]} " in *" -n"*) continue ;; esac
          snapshot=${argv[-1]}
          printf 'zfs_send_running_seconds{snapshot="%s"} %s\n' "$snapshot" "$elapsed" >>"$CHECK_METRICS"
          size=$(zfs send -nP "${argv[@]:2}" 2>/dev/null | awk '$1 == "size" { s = $2 } END { print s }' || true)
          if [ -n "$size" ]; then
            printf 'zfs_send_estimated_bytes{snapshot="%s"} %s\n' "$snapshot" "$size" >>"$CHECK_METRICS"
          fi
          ;;
      esac
    done
    ;;
  capacity)
    pool_re='^(OK|WARNING|CRITICAL) ZPOOL ([^ ]+) : ([0-9]+)%$'
    {
      echo "# HELP sanoid_pool_capacity_percent How full a ZFS pool is, with sanoid's verdict."
      echo "# TYPE sanoid_pool_capacity_percent gauge"
    } >>"$CHECK_METRICS"
    while IFS= read -r msg; do
      if [[ $msg =~ $pool_re ]]; then
        printf 'sanoid_pool_capacity_percent{pool="%s",status="%s"} %s\n' \
          "${BASH_REMATCH[2]}" "$(level "${BASH_REMATCH[1]}")" "${BASH_REMATCH[3]}" >>"$CHECK_METRICS"
      else
        echo "could not parse: $msg" >&2
        unparsed=1
      fi
    done < <(grep -oE '(OK|WARNING|CRITICAL) ZPOOL [^ ]+ : [0-9]+%' <<<"$out" || true)
    ;;
  *)
    echo "unknown mode: $mode" >&2
    exit 2
    ;;
esac

# 0 OK, 1 WARN, 2 CRIT are results; 3 UNKNOWN (or anything else) is not.
if [ "$rc" -gt 2 ] || [ "$unparsed" -ne 0 ]; then
  exit 1
fi
