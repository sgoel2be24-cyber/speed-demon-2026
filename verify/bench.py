#!/usr/bin/env python3
"""Alternate original candidates on the same x86 host; hash every timed output.

Candidate source edits live only in .out/bench/, never in the scored source tree.
These timings are runner measurements, not board-score measurements.
"""
import argparse
import hashlib
import json
import os
import pathlib
import random
import shutil
import statistics
import subprocess
import sys
import time

from candidates import apply

ROOT = pathlib.Path(__file__).resolve().parents[1]


def call(command, **kwargs):
    subprocess.run(command, check=True, **kwargs)


def build(name, variant, training, workspace):
    directory = workspace / name / "track-b"
    directory.mkdir(parents=True)
    # Export committed source so ignored build artifacts cannot affect a candidate.
    archive = subprocess.check_output(["git", "archive", "HEAD:track-b"], cwd=ROOT)
    subprocess.run(["tar", "-x", "-C", str(directory)], input=archive, check=True)
    apply(directory, variant)
    kernel = directory.parent / "verify/kernel_test"
    shutil.copytree(ROOT / "verify/kernel_test", kernel, ignore=shutil.ignore_patterns("target"))
    if "columns" in variant:
        test = kernel / "src/lib.rs"
        test.write_text(test.read_text().replace("assert_eq!(totals.first[0],", "let actual_first: [u16; 10] = std::array::from_fn(|rank| totals.first[rank * totals.count.len()]);\n        assert_eq!(actual_first,"))
    env = dict(os.environ, RUSTFLAGS="-C target-cpu=x86-64-v3", VERIFY_REQUIRE_AVX2="1")
    call(["cargo", "test", "--release", "--", "--nocapture"], cwd=kernel, env=env)
    profile = directory / "profile"
    call(["cargo", "rustc", "--release", "--", "-C", f"profile-generate={profile}"], cwd=directory)
    output = directory / "training-output"
    output.mkdir()
    call([str(directory / "target/release/solution"), str(training), str(output), "12"])
    sysroot = subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip()
    llvm = pathlib.Path(sysroot) / "lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-profdata"
    call([str(llvm), "merge", "-o", str(directory / "profile.profdata"), str(profile)])
    (directory / "src/main.rs").touch()
    call(["cargo", "rustc", "--release", "--", "-C", f"profile-use={directory / 'profile.profdata'}"], cwd=directory)
    return directory / "target/release/solution"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", required=True, type=pathlib.Path)
    parser.add_argument("--train-sample", required=True, type=pathlib.Path)
    parser.add_argument("--train-full", required=True, type=pathlib.Path)
    parser.add_argument("--reference", required=True)
    parser.add_argument("--rounds", type=int, default=9)
    parser.add_argument("--variants", default="baseline,nonzero,sparse,fused,combined")
    parser.add_argument("--full-pgo", default="baseline,combined")
    args = parser.parse_args()
    workspace = ROOT / ".out/bench"
    shutil.rmtree(workspace, ignore_errors=True)
    workspace.mkdir(parents=True)
    binaries = {}
    for variant in args.variants.split(","):
        binaries[variant] = build(variant, variant, args.train_sample.resolve(), workspace)
    for variant in filter(None, args.full_pgo.split(",")):
        binaries[variant + "-full-pgo"] = build(variant + "-full-pgo", variant, args.train_full.resolve(), workspace)
    candidates = [(name, 12) for name in binaries]
    candidates += [(name, threads) for name in ["baseline", "combined", "phase"] if name in binaries for threads in [6, 24]]
    if "columns-hash" in binaries:
        candidates.append(("columns-hash", 6))
    measurements = {f"{name}/{threads}": [] for name, threads in candidates}
    output = workspace / "output"
    output.mkdir()
    expected = {}
    for seed in [2, 3]:
        data = args.data.resolve() / f"full-seed{seed}"
        oracle = data / "oracle/results.txt"
        expected[seed] = hashlib.sha256(oracle.read_bytes()).hexdigest()
        order = candidates.copy()
        rng = random.Random(20261102 + seed)
        for round_no in range(args.rounds + 1):
            rng.shuffle(order)
            for name, threads in order:
                started = time.perf_counter()
                call([str(binaries[name]), str(data), str(output), str(threads)])
                elapsed = time.perf_counter() - started
                digest = hashlib.sha256((output / "results.txt").read_bytes()).hexdigest()
                assert digest == expected[seed], (name, threads, seed, digest, expected[seed])
                if round_no:
                    measurements[f"{name}/{threads}"].append({"seed": seed, "round": round_no, "seconds": elapsed})
            print(f"completed seed {seed}, round {round_no}", flush=True)
    summary = {}
    baseline = measurements["baseline/12"]
    for label, values in measurements.items():
        ratios = []
        medians = {}
        for seed in [2, 3]:
            medians[seed] = statistics.median(v["seconds"] for v in values if v["seed"] == seed)
            base = statistics.median(v["seconds"] for v in baseline if v["seed"] == seed)
            ratios.append(medians[seed] / base)
        summary[label] = {"median_seconds": medians, "mean_ratio_to_baseline": statistics.mean(ratios)}
        print(label, json.dumps(summary[label]), flush=True)
    memory = {}
    for label in measurements:
        name, threads = label.split("/")
        proc = subprocess.Popen([str(binaries[name]), str(args.data.resolve() / "full-seed2"), str(output), threads])
        peak = 0
        while proc.poll() is None:
            try:
                for line in pathlib.Path(f"/proc/{proc.pid}/status").read_text().splitlines():
                    if line.startswith("RssAnon:"):
                        peak = max(peak, int(line.split()[1]))
            except FileNotFoundError:
                pass
            time.sleep(0.001)
        assert proc.returncode == 0
        assert hashlib.sha256((output / "results.txt").read_bytes()).hexdigest() == expected[2]
        memory[label] = {"sampled_peak_RssAnon_MiB": peak / 1024,
                         "scope": "1ms /proc sampling, lower bound; not board cgroup M"}
        print(label, memory[label], flush=True)
    report = {"commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "timing_scope": "x86 runner process wall time, PGO builds; not scoring-box measurements",
              "rounds_per_seed": args.rounds, "expected_sha256": expected,
              "summary": summary, "measurements": measurements, "memory": memory, "all_checks_passed": False}
    (workspace / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    for name, binary in binaries.items():
        call([sys.executable, str(ROOT / "verify/fuzz.py"), "--reference", args.reference,
              "--binary", str(binary), "--rounds", "100", "--seed", "20261102",
              "--keep", str(workspace / f"{name}-failure")])
        call([sys.executable, str(ROOT / "verify/regressions.py"), "--reference", args.reference,
              "--binary", str(binary)])
    report["all_checks_passed"] = True
    (workspace / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print("PASS  all candidate hashes, fuzz and regressions")


if __name__ == "__main__":
    main()
