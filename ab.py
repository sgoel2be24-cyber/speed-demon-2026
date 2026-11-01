"""Build each commit in ab.txt, then alternate runs on seed 2 so all see the same machine."""
import hashlib, os, statistics, subprocess, time
commits = open("ab.txt").read().split()
bins = {}
for c in commits:
    w = f"wt-{c[:8]}"
    subprocess.run(["git", "worktree", "add", "-q", w, c], check=True)
    subprocess.run(["cargo", "build", "--release", "--manifest-path", f"{w}/track-b/Cargo.toml"], check=True, capture_output=True)
    bins[c] = f"{w}/track-b/target/release/solution"
os.makedirs("out", exist_ok=True)
times = {c: [] for c in commits}
hashes = set()
for r in range(21):
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
