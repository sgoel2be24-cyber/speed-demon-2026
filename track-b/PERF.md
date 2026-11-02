# Track B: performance log

Every step from the reference to the final version, each with the live-board run ID that measured it.

| Step | Change | Run ID | T (s) | M (MB) | S | Points |
|---|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 56.35 | 2,200 | 91 | 0 |
| 1 | One pass with no index: scalar tokeniser, byte-hash term table, 64-document blocks | run-4d449ec24411 | 0.582 | 39.4 | 554 | 60.76 |
| 2 | Term table: exact 16-byte keys, multiplicative hash, no token copies | run-258cf7536de6 | 0.437 | 37.9 | 595 | 63.63 |
| 3 | Tokens found 32 bytes at a time with AVX2 | run-6ababf33b096 | 0.365 | 38.4 | 635 | 65.39 |
| 4 | Lookup loop without branches: misses get their own id, the anchor flag lives in the slot | run-d4e0ab16bdba | 0.339 | 38.9 | 653 | 66.09 |
| 5 | Same code in fewer lines | run-b2fced9fd22d | 0.431 | 37.5 | 607 | 63.72 |
| 6 | Evaluate triggered queries in memory order, busiest first | run-7e6f926daaac | 0.295 | 38.6 | 636 | 67.69 |
| 7 | Prefetch the term table's slots, one document at a time | run-a778f48fd086 | 0.294 | 40.1 | 654 | 67.64 |
| 8 | Hash each token once, faster startup, smaller query record, words for BOOL terms only | run-39e060ff4f98 | 0.288 | 37.2 | 664 | 67.79 |
| 9 | Profile-guided build, trained on generator output | run-4edd3e56a048 | 0.280 | 37.5 | 672 | 68.07 |
| 10 | Dense terms, 256-document blocks, faster lookup, larger table on huge pages, parallel output | run-bbc0afa8b489 | 0.242 | 41.5 | 1,013 | 68.33 |

## What the data said before any code

These counts come from replaying the queries against generated data, on seeds 1 and 2, before the design was chosen.

- BOOL terms hold 21.1 M (term, document) pairs, which is 84 MB as plain 32-bit ids. The best bitmap and array split still needed 55 MB or more, and memory stops scoring at 44 MB. That ruled out storing postings.
- Evaluating queries document by document would take 98.6 M evaluations. Evaluating 64 documents at once takes 19 M, because about 3,500 dense queries otherwise dominate.
- Anchoring each phrase on the term that appears least in `queries.txt` needs 1.26 M checks. Always using the first term would need 2,172 M.
- Choosing covers by query-file frequency costs only 3% more evaluations than using true document frequency, so no sampling pass is needed.

## Step 1: one pass, no index

Expected: about 0.25 s on the board, about 36 MB, 330 lines.

Measured before pushing, on an Apple M5, so only the proportions carry over: 0.24 s, 37 MB. Scan CPU split into lookup and word setting 43%, block evaluation 28%, tokenising 27%, phrase checks 2%.

Result: 0.582 s, 39.4 MB, 554 lines, 60.76 points, all gates passed, rank 47 of 101. Memory landed as planned, under the 44 MB cap. Time was 2.3 times slower than estimated, because the estimate priced cache misses far too low.

Lines came out at 554, not 330, which costs about 1.7 points.

## Step 2: term table

Expected: lookup is the largest phase, so a cheaper hash and fewer memory reads should give the biggest single gain.

Measured before pushing, on the Apple M5: CPU time fell 15%, from 1.97 s to 1.67 s, and wall time from 0.24 s to 0.20 s.

Result: 0.437 s, 25% faster, for 41 more lines and a net gain of 2.87 points. Memory fell to 37.9 MB. On the Mac it had looked like a rise.

One trap here: a key holds only 16 bytes, so a 16-byte token could equal a longer term with the same start. Keys of longer tokens carry a marker byte that no token contains, and the fuzz test now includes that exact case.

## Step 3: AVX2 tokeniser

Expected: tokenising was 27% of scan CPU, nearly all of it byte-by-byte work that AVX2 replaces.

Measured before pushing, on the Apple M5, which uses the plain classifier and so does not show the AVX2 gain: CPU time fell a further 13%, to 1.45 s, just from working on whole chunks.

On a GitHub x86-64 runner with AVX2, steps 2 and 3 together took `full` seed 1 from 2.71 s to 1.25 s. That is a slow shared machine, so the ratio matters, not the time.

Result: 0.365 s, 17% faster, 1.76 points. A real gain, but well below the 2.3 times the runner suggested for both steps together. Tokenising was never as large a share on x86 as on the Mac.

## Step 4: lookup without branches

Expected: lookup was still the largest phase. Whether a token is a hit, and whether it is new to the block, are both close to coin flips for the branch predictor, and checking for anchors read a second table.

Measured before pushing, on the Apple M5: CPU time fell from 1.45 s to 1.29 s, and wall time to 0.16 s. On the GitHub x86-64 runner it made no visible difference (1.25 s to 1.23 s), so the board run decides whether this step is worth keeping.

Result: 0.339 s, 7% faster, 0.70 points. Worth keeping, but a smaller step than on the Mac.

The change also caused a bug that only the fuzz test found. Writing `touched` unconditionally means every token writes one entry past the count, so a block containing every term ran off the end of the array. Real data almost never does this, but a tiny fuzz corpus does on its second round.

## Step 5: fewer lines

Expected: no change in speed, about 46 fewer lines.

Result: 0.431 s, far slower than step 4's 0.339 s, for code that does the same work. An A/B run on one x86 machine, alternating the two builds 20 times, put the trim at 1% slower, within noise. So one board run can be off by about a quarter, and a small difference on the board proves nothing. Changes now go through the A/B job first.

## Where the time really goes

The Mac had misled us. Timing each phase on x86 with AVX2 gave a different picture from the Apple M5:

| Phase | Mac share of scan CPU | x86 share of scan CPU |
|---|---|---|
| Block evaluation | 28% | 46% |
| Lookup and word setting | 43% | 33% |
| Tokenising | 27% | 16% |
| Phrase checks | 2% | 3% |

So evaluation, not lookup, is the largest phase on the machine that counts. Each evaluation reads several scattered cache lines, and the M5's large caches hide that cost. From here on, Mac timings only show whether code still works, and x86 runs choose the order of work.

## Step 6: evaluate in memory order

Expected: walking trigger lists jumped around memory, once per query per block. Marking queries in a bitset and then running them in order should turn scattered reads into mostly sequential ones. Sorting queries by how often they run packs the busiest together.

Result: 17% faster in the A/B job, and 0.295 s on the board, a gain of 1.6 points against the best run so far.

## Step 7: prefetch the term table

Expected: rare tokens land on table slots that are not in cache. A document's tokens are known before any lookup, so hashing them first and prefetching their slots lets those misses overlap.

Result: 4.4 to 5.5% faster in two A/B runs. On the board, 0.294 s and 67.64 points, level with step 6, because one run cannot show a 5% gain and the change added 18 lines. Kept on the strength of the A/B runs.

## Step 8: a review of everything before the hot loops

A separate review measured what could be removed before the scan starts, on seeds 2 and 3. Most ideas did not survive contact with the numbers: about 1% of BOOL queries are logically redundant, better covers save only 2.4% of evaluations, and only 22.6% of token hits repeat within a document. Query rewriting was not worth the code.

Five small changes went through one A/B job, each on top of the last:

| Change | Time vs base | This step |
|---|---|---|
| Compute each token's hash once | 0.982 | 1.8% faster |
| Query record 56 to 40 bytes | 0.997 | no real change |
| Smaller parsing table, unstable sorts | 0.969 | 2.8% faster |
| Evaluate the busiest queries in every block | 1.001 | 3.2% slower, removed |
| Words for BOOL terms only | 0.987 | 1.4% faster |

The always-on list had been the review's first choice, on the grounds that it removes two thirds of trigger steps. It evaluates more queries than triggers do, and that cost more than the steps it saved. Its board run was `run-b3099aeb6722` at 0.300 s.

## Step 9: profile-guided build

Expected: evaluation is branch-heavy, so PGO should help. It gave 3.7% in the A/B job.

Result: 0.280 s, the best run so far, for 8 more Dockerfile lines. The generator copy was not counted in S, as the rules promise, and the build took 40 s.

## Step 10: dense terms and 256-document blocks

A longer look at preprocessing found hidden costs a sampling profile cannot show: every token made a real `memcpy` call for 16 bytes, and table lookups were not inlined. It also moved the 5,769 most used terms into a small array, reduced queries to those terms, and enlarged the term table.

Expected: 1.69 times faster, measured in the A/B job as 1,211 ms to 718 ms on a 2-core runner with correct output.

Result: 0.242 s on the board, only 14% faster, for 341 more lines. See the note below on why the A/B job overstated it.

## Experiments that failed

- **Writing token edges in bulk.** Tokenising looked slow for AVX2, so the loop writing edges one at a time was replaced with one writing 16 at a time, to avoid a mispredicted exit per chunk. It came out 2% slower than without it in the A/B job, so it was reverted.
- **Populating the file mapping up front.** The idea was that page faults were a hidden cost. No change in the A/B job, on a 2-core runner. The board has more threads, so it could still differ there.
- **Removing the branch on each BOOL operator.** About 1% in the A/B job, within noise, so not worth the lines.
- **Keeping each query's running counters in one struct.** No real change in the A/B job, so the separate arrays stayed.
- **Computing the key mask instead of reading a table.** About 20 fewer lines but 2.4% slower. The points cancel out, so the table stayed.
- **Removing bounds checks from the hottest loops.** 1.8% faster in the A/B job, but it adds `unsafe` blocks to code people have to review. Not worth it.
- **Prefetching term words too.** A second prefetch stage, with word setting moved to its own pass, came out 2.4% slower in the A/B job. The extra pass cost more than the misses it hid, so it was reverted.

## Profile on x86

A sampling profile of step 7, on a GitHub x86-64 runner, share of CPU time:

| Function | Share |
|---|---|
| Evaluation: `evaluate`, `add_word`, `flush` | 34% |
| Lookup: `find`, `Key::at`, `lookup` | 25.5% |
| Tokenising: `find_token_edges` | 18% |
| Per-token loop: setting term words | 16% |
| Phrase checks | 3% |

Kernel time barely shows, which rules out page faults on this runner. After this, small changes to the existing loops stopped paying, and several made things worse. Going much further needs a different shape for lookup or evaluation, not more tuning. Line count matters as much from here: 100 fewer lines is worth about a point, the same as a 15% speed-up that adds 100 lines.

## Experiments still to try on the board

- Evaluating dense queries on every block from a plain list. Step 6 gets much of the same effect without the extra list.
