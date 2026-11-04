"""Corre el set contra un endpoint /v1/systemone (Ollama local o Jev) y resume.
Uso: python3 -I run.py --base http://localhost:11434 --model NAME [--key-env VAR] --out out.json"""
import argparse, json, os, socket, statistics, sys, time, urllib.request, urllib.error
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cases as C


def call(base, model, key, case):
    body = {"model": model, "state": case["state"], "questions": C.questions(case)}
    h = {"Content-Type": "application/json"}
    if key:
        h["Authorization"] = "Bearer " + key
    req = urllib.request.Request(base + "/v1/systemone", json.dumps(body).encode(), h)
    t = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            out = json.load(r)
    except urllib.error.HTTPError as e:
        return None, (time.perf_counter() - t) * 1000, f"HTTP {e.code}: {e.read()[:200]!r}"
    except Exception as e:  # noqa: BLE001
        return None, (time.perf_counter() - t) * 1000, repr(e)
    return out, (time.perf_counter() - t) * 1000, None


def vault_key(name, tool, domain):
    """Pide la clave al baul por el socket de la pasarela; nunca se imprime."""
    path = os.path.join(os.environ["XDG_RUNTIME_DIR"], "clio", "vault-gateway.sock")
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(10)
        s.connect(path)
        s.sendall((json.dumps({"op": "resolve", "name": name, "tool": tool, "domain": domain}) + "\n").encode())
        r = json.loads(s.makefile().readline())
    if not r.get("ok"):
        sys.exit("el baul no entrego la clave: " + str(r.get("error")))
    d = r["data"]
    return d if isinstance(d, str) else (d.get("value") or d.get("secret") or sys.exit("respuesta del baul inesperada: claves " + str(list(d))))


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(p / 100 * (len(xs) - 1))))]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--model", required=True)
    ap.add_argument("--key-env")
    ap.add_argument("--vault", help="NOMBRE:HERRAMIENTA:DOMINIO")
    ap.add_argument("--out", required=True)
    ap.add_argument("--warmup", type=int, default=2)
    a = ap.parse_args()
    key = os.environ.get(a.key_env) if a.key_env else None
    if a.vault:
        key = vault_key(*a.vault.split(":"))
    cs = C.build()
    for c in cs[: a.warmup]:
        call(a.base, a.model, key, c)  # carga el modelo; no cuenta
    rows = []
    for c in cs:
        out, ms, err = call(a.base, a.model, key, c)
        rows.append({"id": c["id"], "task": c["task"], "ms": ms, "err": err, "answers": (out or {}).get("answers"), "gold": c["gold"], "tokens": ((out or {}).get("usage") or {}).get("input_tokens")})
    json.dump(rows, open(a.out, "w"), ensure_ascii=False, indent=1)
    summarize(a.model, rows)


def summarize(name, rows):
    print(f"\n=== {name} ===")
    errs = [r for r in rows if r["err"]]
    print(f"llamadas: {len(rows)}, con error: {len(errs)}", (errs[0]["err"] if errs else ""))
    ok = [r for r in rows if not r["err"]]
    lat = [r["ms"] for r in ok]
    if lat:
        print(f"latencia ms  p50={pct(lat,50):.0f}  p95={pct(lat,95):.0f}  media={statistics.mean(lat):.0f}")
    for task in ["op", "mem", "del", "tool"]:
        rs = [r for r in ok if r["task"] == task]
        if not rs:
            continue
        hit = tot = 0
        brier = []
        conf_ok, conf_bad = [], []
        by = {}
        for r in rs:
            for q, g in r["gold"].items():
                ans = (r["answers"] or {}).get(q)
                if ans is None:
                    tot += 1
                    continue
                if ans["type"] == "noul":
                    p = ans["noul"]
                    good = (p >= 0.5) == bool(g)
                    brier.append((p - (1.0 if g else 0.0)) ** 2)
                    key = "sí" if g else "no"
                else:
                    accept = g if isinstance(g, list) else [g]
                    good = ans["choice"] in accept
                    (conf_ok if good else conf_bad).append(ans.get("confidence", 0))
                    key = accept[0].split("_")[0]
                tot += 1
                hit += good
                b = by.setdefault(key, [0, 0])
                b[0] += good
                b[1] += 1
        extra = f"  brier={statistics.mean(brier):.3f}" if brier else ""
        if conf_ok or conf_bad:
            extra += f"  conf(acierto)={statistics.mean(conf_ok) if conf_ok else 0:.2f} conf(fallo)={statistics.mean(conf_bad) if conf_bad else 0:.2f}"
        det = "  ".join(f"{k}:{v[0]}/{v[1]}" for k, v in sorted(by.items()))
        print(f"{task:5s} acierto {hit}/{tot} = {100*hit/tot:.0f}%{extra}\n      {det}")


if __name__ == "__main__":
    main()
