import subprocess


def make_archive(name):
    if ";" in name:
        raise ValueError("invalid")
    subprocess.run(f"tar -czf {name}.tar.gz -C data {name}", shell=True, check=True)
