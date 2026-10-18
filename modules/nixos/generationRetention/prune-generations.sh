# Expire old Nix profile generations ahead of nix-gc, never touching the
# current or booted system generation.
#
# Env:
#   OLDER_THAN_DAYS - delete generations older than this many days ("" = off)
#   KEEP            - keep at most this many newest system generations ("" = off)

profiles=/nix/var/nix/profiles
system="$profiles/system"

# --- system profile: explicit generation numbers, minus the protected ones ---

current=$(readlink "$system")
current=${current#system-}
current=${current%-link}

# The booted generation is the known-good fallback: systemd-boot drops its
# entry once the generation is gone, after which systemd-bless-boot fails on
# every switch and there is nothing proven-bootable to roll back to.
booted_path=$(readlink -f /run/booted-system || true)
booted=""

gens=()
for link in "$system"-*-link; do
    [ -L "$link" ] || continue
    n=${link#"$system"-}
    n=${n%-link}
    gens+=("$n")
    target=$(readlink -f "$link")
    if [ "$target" = "$booted_path" ]; then
        booted=$n
        continue
    fi
    # Booted into a specialisation (e.g. recovery) of this generation.
    for spec in "$link"/specialisation/*; do
        if [ -e "$spec" ] && [ "$(readlink -f "$spec")" = "$booted_path" ]; then
            booted=$n
        fi
    done
done

if [ -z "$booted" ]; then
    echo "booted system $booted_path has no generation in $system; nothing to protect"
fi

mapfile -t gens < <(printf '%s\n' "${gens[@]}" | sort -rn)

now=$(date +%s)
rank=0
delete=()
for n in "${gens[@]}"; do
    rank=$((rank + 1))
    if [ "$n" = "$current" ] || [ "$n" = "$booted" ]; then
        continue
    fi
    expired=false
    if [ -n "$OLDER_THAN_DAYS" ]; then
        mtime=$(stat -c %Y "$system-$n-link")
        if [ $((now - mtime)) -gt $((OLDER_THAN_DAYS * 86400)) ]; then
            expired=true
        fi
    fi
    if [ -n "$KEEP" ] && [ "$rank" -gt "$KEEP" ]; then
        expired=true
    fi
    if [ "$expired" = true ]; then
        delete+=("$n")
    fi
done

echo "system: current=$current booted=${booted:-none}"
if [ "${#delete[@]}" -gt 0 ]; then
    echo "system: deleting generations ${delete[*]}"
    nix-env -p "$system" --delete-generations "${delete[@]}"
fi

# --- every other profile: plain age-based expiry, as nix-collect-garbage -d ---

if [ -n "$OLDER_THAN_DAYS" ]; then
    while IFS= read -r profile; do
        [ "$profile" = "$system" ] && continue
        echo "$profile: deleting generations older than ${OLDER_THAN_DAYS}d"
        nix-env -p "$profile" --delete-generations "${OLDER_THAN_DAYS}d"
    done < <(find "$profiles" /root/.local/state/nix/profiles -type l ! -name '*-link' 2>/dev/null)
fi
