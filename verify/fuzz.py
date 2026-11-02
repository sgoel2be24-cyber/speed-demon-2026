#!/usr/bin/env python3
"""Differential fuzz against the reference, aimed at what generated data rarely hits.

    python3 verify/fuzz.py --reference ../speed-demon-2026/reference/b_index.py --rounds 200

Each round writes a small corpus from a tiny vocabulary, so queries really match, using the
awkward cases from TRACK-B-PLAN.md section 3: non-ASCII bytes inside words, 32-byte terms,
tokens at the very end of a file, empty documents first and last, `007` beside `7`, operator
words as terms, repeated runs, and files whose length sits on and around a 4096-byte page
boundary. The same input is then run at several thread counts.
"""
import argparse
import os
import random
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BINARY = os.path.join(ROOT, "track-b", "target", "release", "solution")

WORDS = ["the", "of", "and", "or", "not", "bail", "bond", "writ", "7", "007", "00123", "123", "z", "rich",
         "s", "a" * 16, "a" * 17, "b" * 17, "c" * 31, "d" * 32, "d" * 31,
         "ab" * 8, "ab" * 8 + "c", "ab" * 8 + "d", "e" * 8, "f" * 9, "stanbul", "273", "u", "c"]
ABSENT = ["zurich", "cafe", "273k", "habeascorpus", "andor", "g" * 32, "and0", "x"]
SEPARATORS = [" ", " ", " ", ", ", ". ", "; ", "-", "/", "'s ", "—", " — ", " ", "’s ", " (",
              ") ", "ü", "K", "İ", "\U0001d504", "ﬁ", "...", " & "]
NO_TOKEN = ["", "", "§§ ¶", "* * *", "—", "...", "½"]
DOC_COUNTS = (1, 2, 3, 31, 32, 33, 63, 64, 65, 127, 128, 129,
              255, 256, 257, 511, 512, 513, 767, 768, 769, 1023, 1024, 1025)


def styled(rng, word):
    roll = rng.randrange(6)
    if roll == 0:
        return word.upper()
    if roll == 1:
        return word.title()
    return word


def document(rng):
    roll = rng.randrange(12)
    if roll == 0:
        return rng.choice(NO_TOKEN)
    n = rng.choice((1, 2, 3, 5, 8, 13, 40))
    words = [rng.choice(WORDS) for _ in range(n)]
    if roll == 1:
        words[rng.randrange(n):] = [rng.choice(WORDS)] * rng.randint(2, 5)
    text = styled(rng, words[0])
    for word in words[1:]:
        text += rng.choice(SEPARATORS) + styled(rng, word)
    return text + rng.choice(("", ".", " ", "ü", "?"))


def token_windows(docs):
    import re
    return [re.findall(rb"[a-z0-9]+", d.encode().lower()) for d in docs]


def query(rng, tokens):
    roll = rng.randrange(10)
    if roll < 5:
        terms = [rng.choice(WORDS + ABSENT[:2]) for _ in range(rng.randint(1, 6))]
        if rng.randrange(4) == 0:
            terms[-1] = terms[0]
        line = "BOOL " + terms[0]
        for term in terms[1:]:
            line += f" {rng.choice(('AND', 'OR', 'NOT'))} {term}"
        return line
    source = rng.choice(tokens)
    k = rng.randint(2, 8)
    if len(source) >= k and roll < 9:
        start = rng.randrange(len(source) - k + 1)
        window = [t.decode() for t in source[start:start + k]]
    else:
        window = [rng.choice(WORDS) for _ in range(k)]
    if roll == 7:
        window.reverse()
    if roll == 8:
        window[rng.randrange(k)] = rng.choice(ABSENT)
    if rng.randrange(6) == 0:
        window = [window[0]] * k
    return 'PHRASE "' + " ".join(window) + '"'


def write_case(folder, rng, round_no):
    # Cycle rather than sample: every complete cycle must hit each block boundary.
    n = DOC_COUNTS[round_no % len(DOC_COUNTS)]
    docs = [document(rng) for _ in range(n)]
    tokens = [t for t in token_windows(docs) if t] or [[b"bail"]]
    body = "".join(f"{i}\t{d}\n" for i, d in enumerate(docs)).encode()
    # Every third round, push the end of the file onto or just around a page boundary.
    if round_no % 3 == 0:
        missing = -len(body) % 4096 + rng.choice((-40, -33, -32, -31, -17, -16, -1, 0, 1))
        pad = (b"bail " * (missing // 5 + 1))[:max(missing, 0)]
        body = body[:-1] + pad.rstrip()[:max(missing, 0)] + b"\n"
    with open(os.path.join(folder, "docs.txt"), "wb") as f:
        f.write(body)
    lines = [query(rng, tokens) for _ in range(rng.choice((1, 5, 40, 100, 400)))]
    lines += rng.sample(lines, min(2, len(lines)))
    with open(os.path.join(folder, "queries.txt"), "w") as f:
        f.write("\n".join(lines) + "\n")


def solution(folder, threads, binary, timeout):
    out = tempfile.mkdtemp(prefix="sd-fuzz-")
    proc = subprocess.run([binary, folder, out, str(threads)], capture_output=True, timeout=timeout)
    data = None
    if proc.returncode == 0:
        with open(os.path.join(out, "results.txt"), "rb") as f:
            data = f.read()
    shutil.rmtree(out)
    return data, proc.stderr.decode()[-500:]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--reference", required=True)
    ap.add_argument("--rounds", type=int, default=200)
    ap.add_argument("--seed", type=int, default=2026)
    ap.add_argument("--threads", default="1,3,12")
    ap.add_argument("--binary", default=BINARY)
    ap.add_argument("--timeout", type=float, default=30)
    ap.add_argument("--keep", help="copy the first failing case to this folder")
    args = ap.parse_args()
    rng = random.Random(args.seed)
    folder = tempfile.mkdtemp(prefix="sd-case-")
    for round_no in range(args.rounds):
        write_case(folder, rng, round_no)
        subprocess.run([sys.executable, args.reference, "--data", folder, "--out", folder + "/oracle"], check=True)
        with open(folder + "/oracle/results.txt", "rb") as f:
            want = f.read()
        for threads in (int(t) for t in args.threads.split(",")):
            got, err = solution(folder, threads, args.binary, args.timeout)
            if got != want:
                print(f"FAIL  round {round_no} threads {threads}\n{err}")
                if args.keep:
                    shutil.copytree(folder, args.keep, dirs_exist_ok=True)
                shutil.rmtree(folder)
                sys.exit(1)
    shutil.rmtree(folder)
    print(f"PASS  {args.rounds} rounds, seed {args.seed}, threads {args.threads}; "
          f"{min(args.rounds, len(DOC_COUNTS))} document counts from {DOC_COUNTS}")


if __name__ == "__main__":
    main()
