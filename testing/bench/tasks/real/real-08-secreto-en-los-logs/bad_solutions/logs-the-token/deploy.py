import logging
import os
import sys

log = logging.getLogger("deploy")


def upload(target, token):
    if target == "broken":
        raise RuntimeError(f"upload to {target} rejected (token={token})")
    return f"deployed {target}"


def main(argv):
    logging.basicConfig(stream=sys.stderr, level=logging.DEBUG)
    target = argv[1]
    token = os.environ["DEPLOY_TOKEN"]
    log.debug("deploying %s with token %s", target, token)
    try:
        result = upload(target, token)
    except Exception:
        log.exception("deploy of %s failed", target)
        raise
    print(result)


if __name__ == "__main__":
    main(sys.argv)
