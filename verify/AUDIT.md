# Track B verification audit — 2026-11-02

Branch: `codex/verify-avx2`, based on `f8f1581e5565a9ab7e8feacc6ca428f283478287`.
No push to `speed-demon`, repository-setting change, or speed experiment is part of this audit.

Evidence labels: **measured** means executed checks or inspected recorded measurements;
**read from code** means a source-level conclusion; **hypothesis** means unverified.
Passing finite tests is not a proof for every possible input or the unseen final seed.

## Findings and changes

| Finding | Label | Resolution / limit |
|---|---|---|
| CI built from the repository root and missed `track-b/.cargo/config.toml`, leaving the seed sweep on scalar paths (G1). | Read from code; AVX2 correction measured in CI. | Build inside `track-b/`; assert the compiler reports `target_feature="avx2"`. |
| No dedicated test exercised the AVX2 tally (G1). | Read from code. | `kernel_test` imports the actual entry modules and compares 2,229,920 masks with a bit-by-bit scalar oracle, plus a complete 65,535-document accumulation. All byte values at every byte position and all aligned 16-bit windows and their complements are enumerated; this does not enumerate all 2^256 masks. |
| The exact Dockerfile/PGO build lacked full-seed-2/3 evidence (G2). | Read from code / supplied run history. | `docker.py` builds the unchanged Dockerfile through `check.py`, checks both holdouts in the restricted container, preserves image/binary hashes and outputs, then fuzzes the extracted binary. CI outcome below. |
| Fuzz document/query sizes missed current block boundaries and dense/rare mixes (G3). | Read from code. | Deterministically cycle 24 document counts, including 255/256/257 and 511/512/513; use up to 400 queries and 1/3/12 threads. Added explicit byte-balanced shard/block regressions. |
| CI's evidence upload silently excluded `.out`, so a successful PGO run retained only the compiler feature file. | Measured by downloading run 41407558634's artifact; upload behaviour read from the action documentation. | Enable hidden files for the narrowly scoped PGO/failure paths, then download the artifact in CI and verify both full outputs, the executable hash and completed fuzz/regression statuses. [Action documentation](https://github.com/actions/upload-artifact#uploading-hidden-files). |
| Byte-balanced shards could contain over 65,535 documents even with 12 workers, aborting before output. | Measured. | Reproduced on the old binary with 250,000 documents: 246,000 one-word documents followed by 4,000 999-word documents. The fix reads boundary ids and splits only oversized parts. The corrected binary matches the reference. The clustered distribution is unlike the generator's independent draws; no generated-seed failure was observed. |
| Folded/full queries, dense-term truncation, and stale form operators needed targeted checks. | Measured tests; source interpretation read from code. | Exhausted 18,662 dense/rare patterns and operator sequences through six terms, over every truth assignment. Checked that the folded result is exact wherever no watched rare term triggers evaluation. Forced 65,540 dense candidates and verified the 65,535 cap retains exact ids. No mismatch found. Unused form operators affect sorting only: evaluation stops at `form.len`. |
| `PERF.md` omitted the best run and two identical-source samples; README reused an older CPU share and incorrectly implied every query with dense terms has a folded form (G4/G5). | Measured against recorded board history; stale/incorrect descriptions read from code/docs. | Added all five samples and their provenance, best 0.173507 s / 71.9303 points, median 0.188694 s. Marked old profiling as step 7, explained why `dense AND rare` has no form, removed the rejected experiment from future work, and separated A/B observations from board results. Missing run IDs are identified rather than invented. |
| `sample` PGO training may leave some paths cold; cache contention or static shard imbalance may explain board noise. | Hypothesis. | No training-scale, lookup, table-size or scheduling optimisation performed. Full-seed correctness checks do not establish a speed gain or the cause of timing noise. |
| The new oversized-shard fallback is definitely untrained by PGO's sample corpus: 25,000 documents cannot exceed a 65,535-document shard limit. | Read from code; cold-path correctness measured. | The actual PGO binary passes the 250,000-document clustered regression, which exercises that fallback. This checks correctness of an untrained path, without changing the training scale. |

## Measured checks

Local host: Apple ARM64, Rust 1.99.1, Python 3.12.13. These runs use the scalar path.
Local Docker/Colima was stopped; it was not started or represented as local Docker evidence.

| Build / check | Result |
|---|---|
| Scored source, levels 0–5 and 7, 20 repeats | PASS: worked example, tiny 1–50, sample 1–9, full 1–3, thread counts 1/2/3/7/12 on sample 1. |
| Scored source, expanded fuzz, seed 4242 | PASS: 300 rounds, each at 1/3/12 threads. |
| Corrected source `7b263f2`, same levels and repeats | PASS. |
| Corrected source, expanded fuzz, seed 909 | PASS: 300 rounds, each at 1/3/12 threads. |
| Corrected source, deterministic regressions | PASS: 19 corpora / 22 executions, including local-id limits 65,534/65,535/65,536, exact shard/block boundaries, and the clustered corpus. |
| Six kernel tests, scalar | PASS, including the exhaustive query-pattern checks. |
| AddressSanitizer, corrected source, ARM64 | PASS: all deterministic regressions plus 100 fuzz rounds, seed 20261102, at 1/3/12 threads. Leak checking disabled; address checking enabled. |
| Entry and kernel-crate formatting / Clippy with warnings denied | PASS. |
| Organiser `native_cpu_flags(track-b)` scan | Empty result. |
| Generator byte comparison with organiser commit `9f059f75384607c7836997063b0901b98f36b5fa` | Equal. |

Hosted x86 results are recorded separately from the local scalar checks:

- [Scored-source verification, `d09cb54`, run 41407558634](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41407558634): **PASS**, including AVX2 build assertion, tally/classifier checks, all verification levels, 300 expanded fuzz rounds, actual PGO Docker full seeds 2 and 3, and 300 PGO fuzz rounds. Its artifact omitted hidden `.out` evidence; complete results are in the run log.
- [Corrected-source verification, `7b263f2`, run 41408168101](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41408168101): **PASS** for the full workflow, including all six AVX2 kernel tests, classifier, seed sweep, 300 differential fuzz rounds, deterministic regressions, x86 AddressSanitizer regressions plus 100 fuzz rounds, and the actual PGO image's full-seed checks, 300 fuzz rounds and regressions.
- [Evidence-retention verification, `210856f`, run 41409280070](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41409280070): **PASS** for the complete workflow, including artifact upload/download and hash checks of the retained PGO executable and both full outputs. This has the same corrected entry source tree, `3fea504b6c99f311a2c40a48ed917a137fe86cd0`. Its `verification-evidence` artifact contains `checks.json`, the executable and both full output files.

Reference output hashes (full scale):

```text
seed 1  f86973c210228141bcb6f1f1b10188ad2a9b65f71e7cd7cf4590b38de4e80927
seed 2  7f224ef65795253d56911fa8f25553a8fd0a2cef4addc7bd411e40e8da990869
seed 3  7e004661d6d7b2db15a3171e687d52de1d789f3efe6e32ab4dfe7ad78515a71f
```

`kernel_test` uses ordinary boxed slices as an allocator shim so it tests the actual
query/term/evaluation modules without reproducing the platform mapping code. The
executable and sanitizer checks exercise the real mapping/scan paths. Miri was not run.
PGO fuzz uses the image's extracted executable on the x86 host; full-seed image checks use
`check.py`'s container restrictions. No local/CI wall time is a scoring-box measurement.

## Reproduce

Build from `track-b/`:

```bash
cargo build --release
```

From the repository root (on this Mac, replace `python3` with `~/.local/bin/python3.12 -I`):

```bash
python3 verify/levels.py --data ../.data/b --levels 0,1,2,3,4,5,7 --repeats 20
python3 verify/fuzz.py --reference ../speed-demon-2026/reference/b_index.py --rounds 300 --seed 909
python3 verify/regressions.py --reference ../speed-demon-2026/reference/b_index.py
cargo test --release --manifest-path verify/kernel_test/Cargo.toml -- --nocapture
```

On x86 Linux, enable AVX2 explicitly for the external kernel crate:

```bash
RUSTFLAGS='-C target-cpu=x86-64-v3' VERIFY_REQUIRE_AVX2=1 cargo test --release --manifest-path verify/kernel_test/Cargo.toml -- --nocapture
```

For the image checks, put the pinned organiser checkout at `contest/`, generate data
with `bash verify/generate.sh contest data python3`, and run `python3 verify/docker.py --data data`.
The `.github/workflows/verify.yml` file records the Linux sanitizer commands too.

## Team-owned closeout

- User confirmed in chat on November 2: all team members completed registration (G6). Form responses were not independently inspected.
- Human review of `PERF.md` is pending. Rule 06 expects the log to show a human understood the work; an automated audit cannot attest to this. A teammate should check the actual measurements and explain the cover/folding rules, why the always-on list was rejected, and why a single board timing is insufficient, in their own words.
- New branch code has no board T/M/S result. Do not associate the old 71.93-point run with the shard fix. The team decides whether to promote it before freeze; no merge or push to `speed-demon` was made here.
- Repository visibility and the scheduled publication task were not changed or verified by this audit.
