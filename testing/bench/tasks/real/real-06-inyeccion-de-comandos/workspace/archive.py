import subprocess


def make_archive(name):
    """Create <name>.tar.gz from the folder data/<name>."""
    subprocess.run(f"tar -czf {name}.tar.gz -C data {name}", shell=True, check=True)
