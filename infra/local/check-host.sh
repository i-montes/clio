#!/usr/bin/env bash
# Checks the host that runs Clio (ADR 0006: the user's own machine by default) against S-23, S-38,
# S-39 and S-40. Read-only, no sudo. Prints PASS / WARN / FAIL per check; exits 1 only on FAIL.
# The messages are in English (the person reads them in `clio doctor`); PASS/WARN/FAIL and the S-xx
# keys are the stable part.
#   FAIL = something of Clio is exposed (a Clio process listening outside loopback / tailscale0, or
#          a private folder or file readable by others).
#   WARN = something of the machine that weakens the defense and is the person's call.
# CLIO_DOCTOR_EMBEDDED=1 (set by `clio doctor`): skip the two checks `clio doctor` already does
# itself (Clio's own listeners, S-38, and the permissions of Clio's folders and files, S-23), so
# each one is shown once. Run on its own, the script does them all.
set -uo pipefail
fail=0
embedded="${CLIO_DOCTOR_EMBEDDED:-}"
say() { printf '%-5s %s\n' "$1" "$2"; [ "$1" = FAIL ] && fail=1; return 0; }

# --- S-38: nothing of Clio listens on a public interface -------------------------------------
# Loopback, the tailscale0 interface and the Docker bridge are not public.
clio_public=(); other_public=()
while IFS= read -r line; do
  addr=$(awk '{print $4}' <<<"$line"); proc=$(grep -oE '\(\("[^"]+"' <<<"$line" | head -1 | tr -d '(\"')
  case "$addr" in 127.*|"[::1]"*|*"%lo"*|*"%tailscale0"*|100.*|172.17.*|"[fd7a:"*) continue ;; esac
  if [[ "$proc" == clio* ]]; then clio_public+=("$addr ($proc)"); else other_public+=("$addr${proc:+ ($proc)}"); fi
done < <(ss -ltnpH 2>/dev/null)
if [ -z "$embedded" ]; then
  if [ "${#clio_public[@]}" -gt 0 ]; then say FAIL "a Clio process listens outside loopback and tailscale0: ${clio_public[*]}"
  else say PASS "no Clio process listens on a public interface (S-38)"; fi
fi
[ "${#other_public[@]}" -gt 0 ] && say WARN "other software listens on a public address: ${other_public[*]}"

# --- S-23: Clio's folders and files are private ---------------------------------------------
if [ -z "$embedded" ]; then
  for d in "${XDG_RUNTIME_DIR:-/nonexistent}/clio" "$HOME/.local/share/clio" "$HOME/.config/clio"; do
    [ -d "$d" ] || continue
    mode=$(stat -c %a "$d")
    if [ $((8#$mode & 8#077)) -ne 0 ]; then say FAIL "$d has mode $mode, it must be 700 (S-23)"; else say PASS "$d is private ($mode)"; fi
    while IFS= read -r f; do
      m=$(stat -c %a "$f"); [ $((8#$m & 8#077)) -ne 0 ] && say FAIL "$f has mode $m, it must be 600 (S-23)"
    done < <(find "$d" -type f 2>/dev/null)
  done
fi

# --- S-40: the disk is encrypted ------------------------------------------------------------
src=$(findmnt -no SOURCE -T "$HOME" 2>/dev/null | sed 's/\[.*//')
if [ -n "$src" ] && lsblk -nso TYPE "$src" 2>/dev/null | grep -qx crypt; then say PASS "the disk of \$HOME is encrypted (LUKS) (S-40)"
else say WARN "could not confirm that the disk of \$HOME is encrypted (S-40)"; fi

# --- the front door: firewall and SSH -------------------------------------------------------
fw=""; for u in ufw nftables firewalld; do systemctl is-active --quiet "$u" 2>/dev/null && fw="$u"; done
if [ -n "$fw" ]; then say PASS "firewall active ($fw)"; else say WARN "no firewall service is active (ufw, nftables, firewalld)"; fi
if systemctl is-active --quiet sshd 2>/dev/null || systemctl is-active --quiet ssh 2>/dev/null; then
  say WARN "OpenSSH (sshd) is running; S-38 asks to turn it off or to reach it only through Tailscale"
else say PASS "OpenSSH (sshd) is not running (S-38)"; fi
# Tailscale has its own SSH server inside tailscaled: it needs no sshd and never shows up as a listener.
if command -v tailscale >/dev/null 2>&1 && tailscale debug prefs 2>/dev/null | grep -Eq '"RunSSH": *true'; then
  say WARN "Tailscale SSH is ON: whoever controls a device of your tailnet or your Tailscale account can open a terminal here, if the ssh rules of your ACL allow it. It is reachable only through the tailnet (S-38 is fine). Turn it off with: tailscale set --ssh=false"
fi

# --- Tailscale (S-31) -----------------------------------------------------------------------
if command -v tailscale >/dev/null 2>&1; then
  tailscale status --json 2>/dev/null | python3 -c '
import sys, json
try: d = json.load(sys.stdin)
except Exception: print("WARN tailscale did not answer"); sys.exit()
state = d.get("BackendState"); exp = (d.get("Self") or {}).get("KeyExpiry")
print(("PASS" if state == "Running" else "WARN"), "tailscale is in state", state)
print(("PASS" if exp else "WARN"), "the key of this node " + (f"expires {exp}" if exp else "has no expiry") + " (S-31)")
' | while read -r lvl rest; do say "$lvl" "$rest"; done
  tailscale lock status 2>&1 | grep -q "is NOT enabled" && say WARN "Tailnet Lock is not enabled (S-31): it is optional, your call"
else say WARN "tailscale is not installed"; fi

# --- what limits the damage when the agent runs as the user (S-03, S-39) --------------------
command -v bwrap >/dev/null 2>&1 && say PASS "bwrap is installed: a system-level jail for the commands is possible" || say WARN "bwrap is not installed: there is no system-level jail for the commands yet"
id -nG | tr ' ' '\n' | grep -qx docker && say WARN "your user is in the docker group, which is the same as root: keep Docker out of the agent's reach"
for b in restic borg kopia; do command -v "$b" >/dev/null 2>&1 && have_backup=$b; done
if [ -z "${have_backup:-}" ]; then
  say WARN "no backup tool (restic, borg, kopia): there are no encrypted copies off this machine (S-40)"
elif [ "$have_backup" != restic ]; then
  say PASS "backup tool found: $have_backup (S-40)"
else
  # restic is the one Clio's scripts use (infra/local/backup.unix.sh): is there a repository, and is the last snapshot recent?
  env_file="${CLIO_BACKUP_ENV:-$HOME/.config/clio/backup.env}"
  if [ ! -r "$env_file" ]; then
    say WARN "restic is installed but no repository is set up: run infra/local/backup-init.unix.sh (S-40)"
  else
    # shellcheck source=/dev/null
    last=$( (set -a; . "$env_file"; set +a; restic snapshots --tag clio --latest 1 --json 2>/dev/null) | grep -oE '"time":"[^"]+"' | tail -1 | cut -d'"' -f4)
    if [ -z "$last" ]; then
      say WARN "could not read any snapshot of the backup repository (is the destination disk connected, or are there no copies yet?) (S-40)"
    else
      age=$(( ( $(date +%s) - $(date -d "$last" +%s 2>/dev/null || echo 0) ) / 86400 ))
      if [ "$age" -le 8 ]; then say PASS "last restic backup $age day(s) ago (S-40)"; else say WARN "the last restic backup is $age days old (S-40)"; fi
    fi
    # A repository on this same disk does not survive losing it.
    # shellcheck source=/dev/null
    repo=$( (set -a; . "$env_file"; set +a; printf '%s' "${RESTIC_REPOSITORY:-}") )
    case "$repo" in /*)
      if [ "$(df -P "$(dirname "$repo")" 2>/dev/null | awk 'NR==2{print $1}')" = "$(df -P "$HOME" | awk 'NR==2{print $1}')" ]; then
        say WARN "the backup repository is on the same disk as your data: it protects against deletions, not against losing the disk (S-40)"
      fi ;; esac
  fi
fi

[ "$fail" -eq 0 ] && echo "host: no FAIL" || echo "host: there is a FAIL, see above"
exit "$fail"
