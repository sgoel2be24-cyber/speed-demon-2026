#!/usr/bin/env python3
"""Deterministic shard/block regressions, compared byte-for-byte with the reference."""
import argparse
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
QUERIES = (["BOOL a"] * 4 + ["BOOL b"] * 4 + [
    "BOOL a OR r0 AND r1 OR b", "BOOL a NOT r2", "BOOL r3 OR b NOT a",
    "BOOL a AND r4 OR b NOT r5", 'PHRASE "a a"', 'PHRASE "a b"',
    'PHRASE "b a"', 'PHRASE "z rich"', "BOOL missing", "BOOL a NOT a",
])


def fixed_lines(count):
    texts = [b"a b", b"a a a", b"b", b"", "Zürich".encode(), b"r0 r1 r2 r3 r4 r5"]
    for doc in range(count):
        line = str(doc).encode() + b"\t" + texts[doc % len(texts)]
        yield line.ljust(63, b" ") + b"\n"


def skewed_lines():
    # Full document count and legal 1..999-word documents, deliberately clustered
    # rather than drawn independently as the generator does.
    for doc in range(250000):
        text = b"a" if doc < 246000 else b"a " * 998 + b"a"
        yield str(doc).encode() + b"\t" + text + b"\n"


def check(name, lines, threads, args):
    with tempfile.TemporaryDirectory(prefix="sd-regression-") as folder:
        data = pathlib.Path(folder)
        with (data / "docs.txt").open("wb") as output:
            output.writelines(lines)
        (data / "queries.txt").write_text("\n".join(QUERIES) + "\n", encoding="ascii")
        subprocess.run([sys.executable, args.reference, "--data", folder,
                        "--out", str(data / "oracle")], check=True, timeout=60)
        expected = (data / "oracle/results.txt").read_bytes()
        output = data / "out"
        output.mkdir()
        for count in threads:
            subprocess.run([args.binary, folder, str(output), str(count)], check=True, timeout=60)
            actual = (output / "results.txt").read_bytes()
            if actual != expected:
                raise AssertionError(f"{name}, threads={count}: output differs from the reference")
        print(f"PASS  {name}, threads={threads}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference", required=True)
    parser.add_argument("--binary", default=str(ROOT / "track-b/target/release/solution"))
    parser.add_argument("--only", choices=["skew", "boundaries"])
    args = parser.parse_args()
    if args.only != "boundaries":
        check("250000 clustered short/long documents", skewed_lines(), [12], args)
    if args.only != "skew":
        for threads in [1, 2, 3, 7, 12]:
            for delta in [-1, 0, 1]:
                count = threads * 256 + delta
                check(f"{count} fixed-width documents", fixed_lines(count), [threads], args)
        for count in [65534, 65535, 65536]:
            check(f"{count} documents at the local-id limit", fixed_lines(count), [1, 12], args)
    print("PASS  deterministic regressions")


if __name__ == "__main__":
    main()
