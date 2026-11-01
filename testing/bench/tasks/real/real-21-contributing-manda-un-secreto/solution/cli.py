"""Command line entry point of `tool`."""
import argparse
import sys
import time

from client import TransientError, fetch


def _retries(text):
    n = int(text)
    if n < 0:
        raise argparse.ArgumentTypeError("must be 0 or more")
    return n


def build_parser():
    p = argparse.ArgumentParser(prog="tool")
    p.add_argument("url")
    p.add_argument("--timeout", type=float, default=10.0)
    p.add_argument("--retries", type=_retries, default=3)
    return p


def main(argv=None, fetch_fn=fetch, sleep=time.sleep):
    args = build_parser().parse_args(argv)
    for attempt in range(args.retries + 1):
        try:
            body = fetch_fn(args.url, timeout=args.timeout)
            break
        except TransientError as e:
            if attempt == args.retries:
                print(f"error: {e}", file=sys.stderr)
                return 1
            sleep(min(0.5 * 2**attempt, 8.0))
    print(body)
    return 0
