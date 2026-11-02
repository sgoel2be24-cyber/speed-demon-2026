#!/usr/bin/env python3
"""Check the unchanged board Dockerfile, including PGO, on full seeds 2 and 3.

The extracted image binary also runs the small differential fuzz suite on the x86
host. The full cases still run inside check.py's restricted Docker sandbox.
"""
import argparse
import hashlib
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
TAG = "speeddemon-check/track-b"


def call(*args):
    subprocess.run(args, cwd=ROOT, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", required=True, type=pathlib.Path)
    args = parser.parse_args()
    evidence = ROOT / ".out" / "pgo"
    evidence.mkdir(parents=True, exist_ok=True)
    checks = []
    for seed in (2, 3):
        data = args.data.resolve() / f"full-seed{seed}"
        expected = hashlib.sha256((data / "oracle/results.txt").read_bytes()).hexdigest()
        command = [sys.executable, "check.py", "b", "--data", str(data), "--expected", expected]
        if checks:
            command.append("--skip-build")
        call(*command)
        checks.append({"dataset": data.name, "sha256": expected, "passed": True})
        # Keep both outputs; check.py reuses its output directory for each seed.
        (evidence / f"full-seed{seed}-results.txt").write_bytes(
            (ROOT / ".out/track-b/results.txt").read_bytes())
    container = subprocess.check_output(["docker", "create", TAG], text=True).strip()
    binary = evidence / "solution"
    try:
        call("docker", "cp", f"{container}:/app/solution", str(binary))
    finally:
        call("docker", "rm", container)
    binary.chmod(0o755)
    image = json.loads(subprocess.check_output(["docker", "image", "inspect", TAG], text=True))[0]
    evidence.joinpath("checks.json").write_text(json.dumps({
        "image_id": image["Id"], "architecture": image["Architecture"],
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "full_cases": checks,
    }, indent=2) + "\n")
    call(sys.executable, "verify/fuzz.py", "--reference", "contest/reference/b_index.py",
         "--binary", str(binary), "--rounds", "300", "--seed", "909",
         "--keep", str(evidence / "fuzz-failure"))
    call(sys.executable, "verify/regressions.py", "--reference", "contest/reference/b_index.py",
         "--binary", str(binary))
    print("PASS  PGO image: full seeds 2 and 3, 300 differential fuzz rounds, regressions")


if __name__ == "__main__":
    main()
