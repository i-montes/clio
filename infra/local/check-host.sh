#!/usr/bin/env bash
# Checks the host that runs Clio (ADR 0006: the user's own machine by default) against S-23, S-38,
# S-39 and S-40. Read-only, no sudo. Prints PASS / WARN / FAIL per check; exits 1 only on FAIL.
#   FAIL = something of Clio is exposed (a Clio process listening outside loopback / tailscale0, or
#          a private folder or file readable by others).
#   WARN = something of the machine that weakens the defense and is the person's call.
set -uo pipefail
fail=0
say() { printf '%-5s %s\n' "$1" "$2"; [ "$1" = FAIL ] && fail=1; return 0; }

# --- S-38: nothing of Clio listens on a public interface -------------------------------------
# Loopback, the tailscale0 interface and the Docker bridge are not public.
clio_public=(); other_public=()
while IFS= read -r line; do
  addr=$(awk '{print $4}' <<<"$line"); proc=$(grep -oE '\(\("[^"]+"' <<<"$line" | head -1 | tr -d '(\"')
  case "$addr" in 127.*|"[::1]"*|*"%lo"*|*"%tailscale0"*|100.*|172.17.*|"[fd7a:"*) continue ;; esac
  if [[ "$proc" == clio* ]]; then clio_public+=("$addr ($proc)"); else other_public+=("$addr${proc:+ ($proc)}"); fi
done < <(ss -ltnpH 2>/dev/null)
if [ "${#clio_public[@]}" -gt 0 ]; then say FAIL "a Clio process listens outside loopback and tailscale0: ${clio_public[*]}"
else say PASS "no Clio process listens on a public interface (S-38)"; fi
[ "${#other_public[@]}" -gt 0 ] && say WARN "other software listens on a public address: ${other_public[*]}"

# --- S-23: Clio's folders and files are private ---------------------------------------------
for d in "${XDG_RUNTIME_DIR:-/nonexistent}/clio" "$HOME/.local/share/clio" "$HOME/.config/clio"; do
  [ -d "$d" ] || continue
  mode=$(stat -c %a "$d")
  if [ $((8#$mode & 8#077)) -ne 0 ]; then say FAIL "$d is mode $mode, it must be 700 (S-23)"; else say PASS "$d is private ($mode)"; fi
  while IFS= read -r f; do
    m=$(stat -c %a "$f"); [ $((8#$m & 8#077)) -ne 0 ] && say FAIL "$f is mode $m, it must be 600 (S-23)"
  done < <(find "$d" -type f 2>/dev/null)
done

# --- S-40: the disk is encrypted ------------------------------------------------------------
src=$(findmnt -no SOURCE -T "$HOME" 2>/dev/null | sed 's/\[.*//')
if [ -n "$src" ] && lsblk -nso TYPE "$src" 2>/dev/null | grep -qx crypt; then say PASS "the disk holding \$HOME is encrypted (LUKS) (S-40)"
else say WARN "could not confirm that the disk holding \$HOME is encrypted (S-40)"; fi

# --- the front door: firewall and SSH -------------------------------------------------------
fw=""; for u in ufw nftables firewalld; do systemctl is-active --quiet "$u" 2>/dev/null && fw="$u"; done
if [ -n "$fw" ]; then say PASS "firewall active ($fw)"; else say WARN "no firewall service is active (ufw, nftables, firewalld)"; fi
if systemctl is-active --quiet sshd 2>/dev/null || systemctl is-active --quiet ssh 2>/dev/null; then
  say WARN "an SSH server is running; S-38 wants it off or reachable only through Tailscale"
else say PASS "no SSH server is running (S-38)"; fi

# --- Tailscale (S-31) -----------------------------------------------------------------------
if command -v tailscale >/dev/null 2>&1; then
  tailscale status --json 2>/dev/null | python3 -c '
import sys, json
try: d = json.load(sys.stdin)
except Exception: print("WARN tailscale did not answer"); sys.exit()
state = d.get("BackendState"); exp = (d.get("Self") or {}).get("KeyExpiry")
print(("PASS" if state == "Running" else "WARN"), "tailscale is", state)
print(("PASS" if exp else "WARN"), "this node key " + (f"expires {exp}" if exp else "has no expiry") + " (S-31)")
' | while read -r lvl rest; do say "$lvl" "$rest"; done
  tailscale lock status 2>&1 | grep -q "is NOT enabled" && say WARN "Tailnet Lock is not enabled (S-31): optional, the person decides"
else say WARN "tailscale is not installed"; fi

# --- what limits the damage when the agent runs as the user (S-03, S-39) --------------------
command -v bwrap >/dev/null 2>&1 && say PASS "bwrap is installed: an OS-level jail for commands is possible" || say WARN "bwrap is not installed: no OS-level jail for commands yet"
id -nG | tr ' ' '\n' | grep -qx docker && say WARN "your user is in the docker group, which is root-equivalent: keep Docker out of the agent's reach"
for b in restic borg kopia; do command -v "$b" >/dev/null 2>&1 && have_backup=$b; done
[ -n "${have_backup:-}" ] && say PASS "backup tool found: $have_backup (S-40)" || say WARN "no backup tool (restic, borg, kopia): encrypted off-machine backups (S-40) are not set up"

[ "$fail" -eq 0 ] && echo "host: no FAIL" || echo "host: FAIL, see above"
exit "$fail"
