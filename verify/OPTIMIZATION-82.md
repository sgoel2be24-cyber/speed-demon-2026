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
| Transpose first-ID storage by rank | Measured: 0.4% slower alone; sampled RssAnon falls from 35.7 to 33.1 MiB. | No standalone speed win established. |
| Hash only the first key word | Measured: 2.0% faster; with transposed first-ID storage, 3.0% faster. Full exact-key/token comparisons remain, so collisions cannot change an answer. | Correct hashes/kernel/fuzz/regressions; small candidate needs replication and board evidence. |
| Aligned, reused shared-phase bitmap storage | Measured: still 29.1% slower despite buffer reuse, huge pages and cache-line ownership. | Rejected in this form. |
| Split lookup from mask updates; group queries by active operator shape | Measured: staged lookup 0.9% faster; active operator shape 2.6% faster; both 1.9% faster. | Keep shape grouping as a small candidate; extra staging has no useful established gain. |
| Standard-library SwissTable with exact-key and long-token comparisons | Measured: 36.0% slower, or 26.0% slower with staged lookup. | Rejected. No dependency or approximate-match shortcut was introduced. |
| Cover-selected, short-circuit phase queries | Measured: 21.1% slower; SwissTable combination 51.6% slower. | Rejected at the initial 16K phase size. Smaller aligned phase sizes are in the final comparison. |
| Replicate transposed first IDs plus prefix hash | Measured: 3.4% faster at 12 workers in a second batch. Six-worker combination is 10.3% faster than the 12-worker baseline in that batch (about 1.5% faster than six-worker baseline). | Small repeatable runner candidate; board benefit unmeasured. |
| Add shape ordering to the layout/hash change; retry smaller covered phases | Measured: combined ordering loses 2.1% at twelve workers. Covered 4K/8K/16K phases lose 38.3%/31.0%/26.5%. | Do not retain shape ordering or phase prototypes. |
| Final clean-build replication on a different runner | Measured: hash/layout entry is 0.7% slower at twelve workers and about 1.0% slower than the six-worker control. Six-worker control itself is 6.3% faster than twelve. All hashes/kernel/fuzz/regressions pass. | Hash/layout gain is not portable across these runs. Prefer the verified six-worker control `39386e7` for the next board experiment. Keep `codex/score-82-tuned` experimental. |

Completed experiment runs:

- [Hot loops, run 41430657321](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41430657321); summary in `evidence/hotloop-41430657321.json`.
- [Shared phases, run 41432012014](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41432012014); summary in `evidence/phases-41432012014.json`.
- [Result/hash layout, run 41433476834](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41433476834); summary in `evidence/layout-41433476834.json`.
- [Staged lookup/operator shape, run 41434927534](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41434927534); summary in `evidence/staged-41434927534.json`.
- [Covers and hash maps, run 41436541217](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41436541217); summary in `evidence/covers-hashmaps-41436541217.json`.
- [Combined ordering and phase sizes, run 41438049364](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41438049364); summary in `evidence/combined-41438049364.json`.
- [Actual clean-entry replication, run 41440802609](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41440802609); summary in `evidence/final-replication-41440802609.json`.

The six-worker image candidate `39386e7` passed the full [verification workflow 41434113427](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41434113427), including actual Docker/PGO full seeds 2 and 3. Permission for up to three board samples and restoration if slower was requested in chat; no scoring push is made while that request is unanswered.

The first runner exposes two CPUs, one SMT core, and 32 MiB L3. The scoring box
exposes six P-cores/twelve threads and 24 MiB L3. Six-worker and cache-layout results
therefore need an authorised board experiment before any board improvement is claimed.
The 1 ms RssAnon samples are lower bounds on process anonymous RSS, not the board's cgroup M.

Experiments are generated into `.out/bench/`; rejected implementations are not copied
into `track-b/`. The independently verified control `39386e7` changes the Docker entrypoint
to six workers. The experimental candidate `9b71676` on `codex/score-82-tuned` additionally retains
rank-major first-id storage and the prefix hash; original full-key/long-token comparisons
remain intact. Its [exact-image verification](https://github.com/sgoel2be24-cyber/speed-demon-2026/actions/runs/41440802438)
passed, including AVX2, all seed levels, six-worker fuzz coverage, AddressSanitizer,
actual PGO holdouts and retained evidence. Local scalar levels 0–5/7, 300 fuzz rounds, boundary regressions and strict
Clippy pass. Six-worker execution is now included in routine fuzz/level-7 checks.
The verified twelve-worker version remains recoverable at `4a13116`. The target of 82
has not been demonstrated, and no board score is inferred from CI ratios.
