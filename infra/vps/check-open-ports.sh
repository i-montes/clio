#!/usr/bin/env bash
# S-38 test, NOT RUN YET: from a machine that is NOT in the tailnet, try the ports of the VPS's
# public address. Nothing may answer. Usage: infra/vps/check-open-ports.sh <public-ip>
set -euo pipefail
ip=${1:?usage: check-open-ports.sh <public-ip>}
open=()
for port in $(seq 1 1024) 2222 3000 3306 5432 6379 8000 8080 8443 9000 41641; do
  if timeout 1 bash -c "echo > /dev/tcp/$ip/$port" 2>/dev/null; then open+=("$port"); fi
done
if [ "${#open[@]}" -gt 0 ]; then
  echo "FAIL: open ports on $ip: ${open[*]}" >&2
  exit 1
fi
echo "ok: no TCP port answered on $ip (1-1024 and a few usual ones)"
