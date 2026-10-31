#!/usr/bin/env python3
"""Checks the versioned system prompts (S-09): naming, the manifest, and nothing in them that would
matter if it leaked. `--update` rewrites MANIFEST.json; `--self-test` checks the checker."""
import hashlib
import json
import os
import re
import sys

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "prompts")
NAME = re.compile(r"^([a-z0-9-]+)@([0-9]+)\.md$")
FORBIDDEN = [
    ("an email address", re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")),
    ("a path on a machine", re.compile(r"(/home/|/Users/|C:\\Users\\)")),
    ("an IPv4 address", re.compile(r"\b[0-9]{1,3}(\.[0-9]{1,3}){3}\b")),
    ("a key or token prefix", re.compile(r"(\bsk-[A-Za-z0-9_-]{8,}|\bAKIA[0-9A-Z]{8,}|\bghp_[A-Za-z0-9]{8,}|\bxox[abprs]-|Bearer +[A-Za-z0-9._-]{8,}|-----BEGIN)")),
    ("a long random-looking string", re.compile(r"[A-Za-z0-9+/=_-]{32,}")),
]


def problems_in(text: str, name: str) -> list:
    out = []
    m = NAME.match(name)
    if not m:
        return [f"{name}: the name must look like <name>@<version>.md"]
    first = text.split("\n", 1)[0].strip()
    if first != f"version: {m.group(2)}":
        out.append(f"{name}: the first line must be 'version: {m.group(2)}', not {first!r}")
    for label, pattern in FORBIDDEN:
        if pattern.search(text):
            out.append(f"{name}: contains {label}")
    return out


def digest(path: str) -> str:
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


def check(update: bool) -> int:
    folder = os.path.join(ROOT, "system")
    files = sorted(f for f in os.listdir(folder) if f.endswith(".md")) if os.path.isdir(folder) else []
    problems = []
    for f in files:
        problems += problems_in(open(os.path.join(folder, f)).read(), f)
    manifest_path = os.path.join(ROOT, "MANIFEST.json")
    current = {f: digest(os.path.join(folder, f)) for f in files}
    if update and not problems:
        json.dump(current, open(manifest_path, "w"), indent=2, sort_keys=True)
        open(manifest_path, "a").write("\n")
        print(f"manifest updated: {len(files)} prompt(s)")
        return 0
    recorded = json.load(open(manifest_path)) if os.path.exists(manifest_path) else {}
    for f, d in current.items():
        if f not in recorded:
            problems.append(f"{f}: not in MANIFEST.json (run with --update)")
        elif recorded[f] != d:
            problems.append(f"{f}: changed without updating MANIFEST.json; a published version is never edited, add a new one")
    for f in recorded:
        if f not in current:
            problems.append(f"{f}: in MANIFEST.json but the file is gone")
    for p in problems:
        print("error:", p, file=sys.stderr)
    print(f"prompts: {len(files)} checked, {len(problems)} problem(s)")
    return 1 if problems or not files else 0


def self_test() -> int:
    bad = {
        "an email": "version: 1\nwrite to ana@example.com",
        "a home path": "version: 1\nsee /home/ana/notes",
        "an ip": "version: 1\nserver 10.1.2.3",
        "a key": "version: 1\nsk-" + "a1B2c3D4e5F6",
        "a long string": "version: 1\n" + "Zx9" * 14,
    }
    for label, text in bad.items():
        assert problems_in(text, "t@1.md"), f"did not flag {label}"
    assert not problems_in("version: 1\nBe short and concrete.", "t@1.md")
    assert problems_in("version: 2\nx", "t@1.md"), "did not flag a version mismatch"
    assert problems_in("version: 1\nx", "t.md"), "did not flag a bad name"
    print("self-test ok")
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    sys.exit(check("--update" in sys.argv))
