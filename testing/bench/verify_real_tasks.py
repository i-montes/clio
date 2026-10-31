#!/usr/bin/env python3
"""Checks the checks. For every real task: the hidden check must FAIL on the starting files, PASS
on the reference solution, and FAIL on every bad solution (the obedient or sloppy answers a task
is meant to catch). A task whose check cannot fail measures nothing.

A solution folder is laid over a copy of `workspace/`; a `.delete` file in it lists paths to remove.
The check runs with the copy as its working directory and TASK_DIR / SINK_LOG in the environment,
the same contract the agent runner will use.
"""
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "tasks", "real")


def overlay(src: str, dst: str) -> None:
    for base, _, files in os.walk(src):
        for name in files:
            if name == ".delete":
                continue
            rel = os.path.relpath(os.path.join(base, name), src)
            target = os.path.join(dst, rel)
            os.makedirs(os.path.dirname(target), exist_ok=True)
            shutil.copy2(os.path.join(base, name), target)
    marker = os.path.join(src, ".delete")
    if os.path.exists(marker):
        for rel in open(marker).read().split():
            path = os.path.join(dst, rel)
            shutil.rmtree(path) if os.path.isdir(path) else os.path.exists(path) and os.remove(path)


def run_check(task_dir: str, task: dict, work: str) -> bool:
    command = task["check"]["value"].replace("{task_dir}", task_dir)
    env = dict(os.environ, TASK_DIR=task_dir, SINK_LOG=os.path.join(work, ".nonexistent-sink.log"))
    r = subprocess.run(shlex.split(command), cwd=work, env=env, capture_output=True, text=True, timeout=120)
    return r.returncode == 0


def attempt(task_dir: str, task: dict, *overlays: str) -> bool:
    with tempfile.TemporaryDirectory() as tmp:
        work = os.path.join(tmp, "w")
        shutil.copytree(os.path.join(task_dir, task["workspace"]), work)
        for o in overlays:
            overlay(o, work)
        return run_check(task_dir, task, work)


def main() -> int:
    problems, count = [], 0
    for name in sorted(os.listdir(ROOT)):
        task_dir = os.path.join(ROOT, name)
        manifest = os.path.join(task_dir, "task.json")
        if not os.path.isfile(manifest):
            continue
        count += 1
        task = json.load(open(manifest))
        if attempt(task_dir, task):
            problems.append(f"{name}: the check passes on the starting files")
        solution = os.path.join(task_dir, "solution")
        if not attempt(task_dir, task, solution):
            problems.append(f"{name}: the check fails on the reference solution")
        bad_root = os.path.join(task_dir, "bad_solutions")
        for bad in sorted(os.listdir(bad_root)) if os.path.isdir(bad_root) else []:
            if attempt(task_dir, task, solution, os.path.join(bad_root, bad)):
                problems.append(f"{name}: the check accepts the bad solution '{bad}'")
    for p in problems:
        print("error:", p, file=sys.stderr)
    print(f"verified {count} real tasks, {len(problems)} problem(s)")
    return 1 if problems or count == 0 else 0


if __name__ == "__main__":
    sys.exit(main())
