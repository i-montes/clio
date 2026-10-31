#!/usr/bin/env bash
# Checks the host that runs Clio (ADR 0006: the user's own machine by default) against S-23, S-38,
# S-39 and S-40. Read-only, no sudo. Prints PASS / WARN / FAIL per check; exits 1 only on FAIL.
# The messages are in Spanish (the person reads them in `clio doctor`); PASS/WARN/FAIL and the S-xx
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
  if [ "${#clio_public[@]}" -gt 0 ]; then say FAIL "un proceso de Clio escucha fuera de loopback y de tailscale0: ${clio_public[*]}"
  else say PASS "ningún proceso de Clio escucha en una interfaz pública (S-38)"; fi
fi
[ "${#other_public[@]}" -gt 0 ] && say WARN "otro software escucha en una dirección pública: ${other_public[*]}"

# --- S-23: Clio's folders and files are private ---------------------------------------------
if [ -z "$embedded" ]; then
  for d in "${XDG_RUNTIME_DIR:-/nonexistent}/clio" "$HOME/.local/share/clio" "$HOME/.config/clio"; do
    [ -d "$d" ] || continue
    mode=$(stat -c %a "$d")
    if [ $((8#$mode & 8#077)) -ne 0 ]; then say FAIL "$d tiene modo $mode, debe ser 700 (S-23)"; else say PASS "$d es privada ($mode)"; fi
    while IFS= read -r f; do
      m=$(stat -c %a "$f"); [ $((8#$m & 8#077)) -ne 0 ] && say FAIL "$f tiene modo $m, debe ser 600 (S-23)"
    done < <(find "$d" -type f 2>/dev/null)
  done
fi

# --- S-40: the disk is encrypted ------------------------------------------------------------
src=$(findmnt -no SOURCE -T "$HOME" 2>/dev/null | sed 's/\[.*//')
if [ -n "$src" ] && lsblk -nso TYPE "$src" 2>/dev/null | grep -qx crypt; then say PASS "el disco de \$HOME está cifrado (LUKS) (S-40)"
else say WARN "no se pudo confirmar que el disco de \$HOME esté cifrado (S-40)"; fi

# --- the front door: firewall and SSH -------------------------------------------------------
fw=""; for u in ufw nftables firewalld; do systemctl is-active --quiet "$u" 2>/dev/null && fw="$u"; done
if [ -n "$fw" ]; then say PASS "cortafuegos activo ($fw)"; else say WARN "ningún servicio de cortafuegos está activo (ufw, nftables, firewalld)"; fi
if systemctl is-active --quiet sshd 2>/dev/null || systemctl is-active --quiet ssh 2>/dev/null; then
  say WARN "OpenSSH (sshd) está corriendo; S-38 pide apagarlo o que solo se alcance por Tailscale"
else say PASS "OpenSSH (sshd) no está corriendo (S-38)"; fi
# Tailscale has its own SSH server inside tailscaled: it needs no sshd and never shows up as a listener.
if command -v tailscale >/dev/null 2>&1 && tailscale debug prefs 2>/dev/null | grep -Eq '"RunSSH": *true'; then
  say WARN "Tailscale SSH está ACTIVO: quien controle un dispositivo de tu tailnet o tu cuenta de Tailscale puede abrir una terminal aquí, si las reglas ssh de tu ACL lo permiten. Solo se alcanza por la tailnet (S-38 bien). Apágalo con: tailscale set --ssh=false"
fi

# --- Tailscale (S-31) -----------------------------------------------------------------------
if command -v tailscale >/dev/null 2>&1; then
  tailscale status --json 2>/dev/null | python3 -c '
import sys, json
try: d = json.load(sys.stdin)
except Exception: print("WARN tailscale no respondió"); sys.exit()
state = d.get("BackendState"); exp = (d.get("Self") or {}).get("KeyExpiry")
print(("PASS" if state == "Running" else "WARN"), "tailscale está en estado", state)
print(("PASS" if exp else "WARN"), "la llave de este nodo " + (f"vence {exp}" if exp else "no tiene vencimiento") + " (S-31)")
' | while read -r lvl rest; do say "$lvl" "$rest"; done
  tailscale lock status 2>&1 | grep -q "is NOT enabled" && say WARN "Tailnet Lock no está activado (S-31): es opcional, lo decides tú"
else say WARN "tailscale no está instalado"; fi

# --- what limits the damage when the agent runs as the user (S-03, S-39) --------------------
command -v bwrap >/dev/null 2>&1 && say PASS "bwrap está instalado: es posible una jaula a nivel del sistema para los comandos" || say WARN "bwrap no está instalado: todavía no hay jaula a nivel del sistema para los comandos"
id -nG | tr ' ' '\n' | grep -qx docker && say WARN "tu usuario está en el grupo docker, que equivale a root: mantén Docker fuera del alcance del agente"
for b in restic borg kopia; do command -v "$b" >/dev/null 2>&1 && have_backup=$b; done
[ -n "${have_backup:-}" ] && say PASS "herramienta de copias de seguridad encontrada: $have_backup (S-40)" || say WARN "no hay herramienta de copias de seguridad (restic, borg, kopia): no hay copias cifradas fuera del equipo (S-40)"

[ "$fail" -eq 0 ] && echo "equipo: sin FAIL" || echo "equipo: hay FAIL, mira arriba"
exit "$fail"
