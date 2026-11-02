# Track B: 82-point optimisation attempt

The measured board baseline remains 71.9303 points, T = 0.173507 s (`f8f1581`).
At unchanged memory/source size, 82 needs approximately 0.069 s, about 2.5 times faster.
No scoring-branch push or repository-setting change is authorised by this work so far.

## Evidence and decisions

All findings are labelled **measured**, **read from code**, or **hypothesis**.
Benchmark timings are process wall times on an x86 runner, not scoring-box times.
Every timed output is hashed against the organiser oracle on full seeds 2 and 3.
Each candidate also runs kernel tests, 100 differential fuzz rounds and deterministic regressions.

| Experiment | Finding | Disposition |
|---|---|---|
| Skip zero folded results | Measured: 3.8% slower averaged across both seeds. | Rejected. |
| Sparse tally for up to eight set bits | Measured: 13.0% slower. | Rejected. |
| Fuse key construction/prefetch into token-edge discovery | Measured: 6.4% slower. | Rejected. |
| Combine the three changes | Measured: 16.8% slower. | Rejected. |
| Train PGO on full rather than sample | Measured: baseline +2.1% in batch 1, -1.3% in batch 2. No repeatable gain established. | Keep sample training. |
| Shared dense bitmaps plus rare postings, 4K/16K/32K-document phases | Measured: 56.7%/37.5%/36.8% slower, respectively. Correct hashes and fuzz/regressions. | Rejected in this form. |
| Six rather than twelve scan workers | Measured: 6.6% faster in batch 1 and 11.2% in batch 2. Sampled peak RssAnon about 24.2 MiB versus 35.9 MiB in batch 2. | Exact six-worker Docker candidate `39386e7` is being verified on `codex/score-82`; board benefit remains a hypothesis. |
| Transpose first-ID storage by rank | Read from code: current row layout spreads rare first matches across 20-byte records. Hypothesis: a column layout reduces the active cache footprint. | Local full-seed 2/3 hashes and fuzz pass; x86 batch pending. |
| Hash only the first key word | Read from code: full exact-key/token comparisons remain, so hash collisions cannot change answers. Hypothesis: fewer hash instructions may help. | Local hashes and fuzz pass; x86 batch pending. |
| Aligned, reused shared-phase bitmap storage | Read from code: original prototype reallocates per phase and worker boundaries can share cache lines. Hypothesis: reuse, huge pages and 512-document ownership boundaries reduce overhead. | Local hashes/fuzz/regressions pass; x86 batch pending. |
| Split lookup from mask updates; group queries by active operator shape | Read from disassembly/code: the current combined loop reloads several spilled pointers; unused form operators affect sorting. Hypothesis: narrower loops and shape grouping help. | Local full-seed 2/3 hashes and fuzz pass; x86 batch being prepared. |

First two completed runs:

- [Hot loops, run 41430657321](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41430657321); summary in `evidence/hotloop-41430657321.json`.
- [Shared phases, run 41432012014](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41432012014); summary in `evidence/phases-41432012014.json`.

The first runner exposes two CPUs, one SMT core, and 32 MiB L3. The scoring box
exposes six P-cores/twelve threads and 24 MiB L3. Six-worker and cache-layout results
therefore need an authorised board experiment before any board improvement is claimed.
The 1 ms RssAnon samples are lower bounds on process anonymous RSS, not the board's cgroup M.

Experiments are generated into `.out/bench/`; rejected implementations are not copied
into `track-b/`. The only scoring candidate change so far is the explicit six-worker
Docker entrypoint. The verified twelve-worker version remains recoverable at `4a13116`.
