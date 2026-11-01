"""Build each commit in ab.txt, then alternate runs on seed 2 so all see the same machine."""
import hashlib, os, statistics, subprocess, time
commits = open("ab.txt").read().split()
bins = {}
SYS = subprocess.run(["rustc", "--print", "sysroot"], capture_output=True, text=True).stdout.strip()
PROFDATA = f"{SYS}/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-profdata"
if any(c.endswith(":pgo") for c in commits):
    subprocess.run(["rustup", "component", "add", "llvm-tools"], check=True)
    subprocess.run(["python", "contest/generators/b_index.py", "--seed", "7", "--scale", "sample", "--out", "train"], check=True)
for c in commits:
    sha = c.split(":")[0]
    w = f"wt-{c.replace(':', '-')[:12]}"
    subprocess.run(["git", "worktree", "add", "-q", "--detach", w, sha], check=True)
    d = os.path.abspath(f"{w}/track-b")
    if c.endswith(":pgo"):
        prof = os.path.abspath(f"pgo-{sha[:7]}")
        subprocess.run(["cargo", "rustc", "--release", "--", "-C", f"profile-generate={prof}"], cwd=d, check=True, capture_output=True)
        os.makedirs("trainout", exist_ok=True)
        subprocess.run([f"{d}/target/release/solution", "train", "trainout", "12"], check=True)
        subprocess.run([PROFDATA, "merge", "-o", f"{prof}.profdata", prof], check=True)
        subprocess.run(["touch", f"{d}/src/main.rs"], check=True)
        subprocess.run(["cargo", "rustc", "--release", "--", "-C", f"profile-use={prof}.profdata"], cwd=d, check=True, capture_output=True)
    else:
        subprocess.run(["cargo", "build", "--release"], cwd=d, check=True, capture_output=True)
    bins[c] = f"{d}/target/release/solution"
os.makedirs("out", exist_ok=True)
times = {c: [] for c in commits}
hashes = set()
for r in range(13):
    for c in commits:
        t = time.monotonic()
        subprocess.run([bins[c], "data", "out", "4"], check=True)
        times[c].append(time.monotonic() - t)
        if r == 0:
            hashes.add(hashlib.sha256(open("out/results.txt", "rb").read()).hexdigest())
print("result hashes", hashes)
base = None
for c in commits:
    v = sorted(times[c][1:])
    m = statistics.median(v)
    base = base or m
    print(f"{c[:8]} median {m*1000:7.1f} ms  min {v[0]*1000:7.1f} ms  ratio {m/base:.3f}")
