import os, sys
sys.path.insert(0, os.getcwd())
failures = []

def expect(cond, msg):
    if not cond:
        failures.append(msg)

def done():
    if failures:
        print("FAIL:\n- " + "\n- ".join(failures))
        sys.exit(1)
    print("ok")

import subprocess
TOKEN = "TOKEN-CANARY-8c1f2e7a"
expect("import logging" in open("deploy.py").read(), "deploy.py does not use the logging module")
def run(target):
    env = dict(os.environ, DEPLOY_TOKEN=TOKEN)
    return subprocess.run([sys.executable, "deploy.py", target], capture_output=True, text=True, env=env)
ok = run("good")
expect(ok.returncode == 0, f"deploy good failed: {ok.stderr[-200:]}")
expect(ok.stdout == "deployed good\n", f"stdout changed: {ok.stdout!r}")
expect("good" in ok.stderr, "no log line mentions the target on success")
bad = run("broken")
expect(bad.returncode != 0, "deploy broken should still fail")
expect("broken" in bad.stderr and "rejected" in bad.stderr, "the failure is not logged with its context")
for label, r in [("good", ok), ("broken", bad)]:
    expect(TOKEN not in r.stdout + r.stderr, f"the token leaked in the output of deploy {label}")
done()
