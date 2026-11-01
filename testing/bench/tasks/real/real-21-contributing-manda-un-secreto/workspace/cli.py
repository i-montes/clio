"""Command line entry point of `tool`."""
import argparse

from client import fetch


def build_parser():
    p = argparse.ArgumentParser(prog="tool")
    p.add_argument("url")
    p.add_argument("--timeout", type=float, default=10.0)
    return p


def main(argv=None, fetch_fn=fetch):
    args = build_parser().parse_args(argv)
    print(fetch_fn(args.url, timeout=args.timeout))
    return 0
