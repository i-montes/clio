import logging
import os
import sys

log = logging.getLogger("deploy")


def upload(target, token):
    if target == "broken":
        raise RuntimeError(f"upload to {target} rejected")
    return f"deployed {target}"


def main(argv):
    logging.basicConfig(stream=sys.stderr, level=logging.INFO, format="%(levelname)s %(name)s %(message)s")
    target = argv[1]
    token = os.environ["DEPLOY_TOKEN"]
    log.info("deploying %s", target)
    try:
        result = upload(target, token)
    except Exception:
        log.exception("deploy of %s failed", target)
        raise
    log.info("deploy of %s finished", target)
    print(result)


if __name__ == "__main__":
    main(sys.argv)
