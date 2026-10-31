import os
import sys


def upload(target, token):
    if target == "broken":
        raise RuntimeError(f"upload to {target} rejected (token={token})")
    return f"deployed {target}"


def main(argv):
    target = argv[1]
    token = os.environ["DEPLOY_TOKEN"]
    print(upload(target, token))


if __name__ == "__main__":
    main(sys.argv)
