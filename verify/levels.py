#!/usr/bin/env python3
"""Run the solution against the reference, level by level. See TRACK-B-PLAN.md section 4.

    python3 verify/levels.py --data ../.data/b --levels 0,1,2,3,4
    python3 verify/levels.py --data ../.data/b --levels 7 --threads 1,2,3,7,12 --repeats 20

Each dataset folder holds docs.txt, queries.txt and oracle/results.txt from the reference.
Prints the first differing line and its query on a mismatch, and exits 1 on any failure.
"""
import argparse
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BINARY = os.path.join(ROOT, "track-b", "target", "release", "solution")
PUBLISHED = {
    "sample-seed1": "eeeec1e9319da98a6f3c664c78f93e7078fd603c25c2bb3027174e81e3638a8f",
    "full-seed1": "f86973c210228141bcb6f1f1b10188ad2a9b65f71e7cd7cf4590b38de4e80927",
}
LEVELS = {
    1: [f"tiny-seed{s}" for s in range(1, 51)],
    2: ["sample-seed1"],
    3: ["full-seed1"],
    4: ["full-seed2"],
    5: ["full-seed3"] + [f"sample-seed{s}" for s in range(2, 10)],
}


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def first_difference(got_path, want_path, data):
    got = open(got_path, "rb").read().split(b"\n")
    want = open(want_path, "rb").read().split(b"\n")
    queries = open(os.path.join(data, "queries.txt"), "rb").read().split(b"\n")
    for i, (g, w) in enumerate(zip(got, want)):
        if g != w:
            q = queries[i].decode() if i < len(queries) else "(past the last query)"
            return f"line {i + 1}: {q}\n      got  {g.decode()[:100]}\n      want {w.decode()[:100]}"
    return f"line counts differ: got {len(got)}, want {len(want)}"


def run(data, threads=None):
    """Run the binary once. Returns (seconds, output folder)."""
    out = tempfile.mkdtemp(prefix="sd-")
    cmd = [BINARY, data, out] + ([str(threads)] if threads else [])
    started = time.monotonic()
    proc = subprocess.run(cmd, capture_output=True)
    took = time.monotonic() - started
    if proc.returncode:
        print(f"FAIL  {data}: exit {proc.returncode}\n{proc.stderr.decode()[-800:]}")
        return took, None
    if os.listdir(out) != ["results.txt"]:
        print(f"FAIL  {data}: output folder holds {os.listdir(out)}")
        return took, None
    return took, out


def check(data, name, threads=None):
    expected = PUBLISHED.get(name)
    oracle = os.path.join(data, "oracle", "results.txt")
    if expected is None:
        expected = sha256(oracle)
    took, out = run(data, threads)
    if out is None:
        return False
    result = os.path.join(out, "results.txt")
    ok = sha256(result) == expected
    label = f"{name}" + (f" threads={threads}" if threads else "")
    if ok:
        print(f"ok    {label}  {took * 1000:.0f} ms")
    else:
        print(f"FAIL  {label}\n      {first_difference(result, oracle, data)}")
    shutil.rmtree(out)
    return ok


def worked_example():
    folder = os.path.join(ROOT, "verify", "worked_example")
    took, out = run(folder)
    if out is None:
        return False
    got = open(os.path.join(out, "results.txt"), "rb").read()
    want = open(os.path.join(folder, "expected.txt"), "rb").read()
    shutil.rmtree(out)
    print(("ok    " if got == want else "FAIL  ") + f"worked example  {took * 1000:.0f} ms")
    return got == want


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True, help="folder holding tiny-seedN, sample-seedN, full-seedN")
    ap.add_argument("--levels", default="0,1,2,3,4")
    ap.add_argument("--threads", default="1,2,3,7,12", help="thread counts for level 7")
    ap.add_argument("--repeats", type=int, default=20, help="repeat runs for level 7")
    args = ap.parse_args()
    if not os.path.exists(BINARY):
        sys.exit(f"build first: {BINARY} is missing")
    failures = 0
    for level in (int(x) for x in args.levels.split(",")):
        if level == 0:
            failures += not worked_example()
        elif level in LEVELS:
            for name in LEVELS[level]:
                failures += not check(os.path.join(args.data, name), name)
        elif level == 7:
            data = os.path.join(args.data, "sample-seed1")
            for threads in (int(x) for x in args.threads.split(",")):
                failures += not check(data, "sample-seed1", threads)
            for _ in range(args.repeats):
                failures += not check(data, "sample-seed1")
        else:
            sys.exit(f"unknown level {level}")
    print("PASS" if failures == 0 else f"FAILED {failures}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
