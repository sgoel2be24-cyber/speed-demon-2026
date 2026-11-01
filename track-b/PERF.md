# Track B: performance log

Every step from the reference to the final version, each with the live-board run ID that measured it.

| Step | Change | Run ID | T (s) | M (MB) | S | Points |
|---|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 56.35 | 2,200 | 91 | 0 |
| 1 | One pass with no index: scalar tokeniser, byte-hash term table, 64-document blocks | | | | | |
| 2 | Term table: exact 16-byte keys, multiplicative hash, no token copies | | | | | |
| 3 | Tokens found 32 bytes at a time with AVX2 | | | | | |
| 4 | Lookup loop without branches: misses get their own id, the anchor flag lives in the slot | | | | | |

## What the data said before any code

These counts come from replaying the queries against generated data, on seeds 1 and 2, before the design was chosen.

- BOOL terms hold 21.1 M (term, document) pairs, which is 84 MB as plain 32-bit ids. The best bitmap and array split still needed 55 MB or more, and memory stops scoring at 44 MB. That ruled out storing postings.
- Evaluating queries document by document would take 98.6 M evaluations. Evaluating 64 documents at once takes 19 M, because about 3,500 dense queries otherwise dominate.
- Anchoring each phrase on the term that appears least in `queries.txt` needs 1.26 M checks. Always using the first term would need 2,172 M.
- Choosing covers by query-file frequency costs only 3% more evaluations than using true document frequency, so no sampling pass is needed.

## Step 1: one pass, no index

Expected: about 0.25 s on the board, about 36 MB, 330 lines.

Measured before pushing, on an Apple M5, so only the proportions carry over: 0.24 s, 37 MB. Scan CPU split into lookup and word setting 43%, block evaluation 28%, tokenising 27%, phrase checks 2%.

Result: to be filled in from the first board run.

Lines came out about 550, not 330, which costs about 1.7 points. Lookup is the largest phase, so the term table comes before AVX2.

## Step 2: term table

Expected: lookup is the largest phase, so a cheaper hash and fewer memory reads should give the biggest single gain.

Measured before pushing, on the Apple M5: CPU time fell 15%, from 1.97 s to 1.67 s, and wall time from 0.24 s to 0.20 s. Peak memory rose from 37 MB to about 40 MB.

Result: to be filled in from the board run.

One trap here: a key holds only 16 bytes, so a 16-byte token could equal a longer term with the same start. Keys of longer tokens carry a marker byte that no token contains, and the fuzz test now includes that exact case.

## Step 3: AVX2 tokeniser

Expected: tokenising was 27% of scan CPU, nearly all of it byte-by-byte work that AVX2 replaces.

Measured before pushing, on the Apple M5, which uses the plain classifier and so does not show the AVX2 gain: CPU time fell a further 13%, to 1.45 s, just from working on whole chunks.

On a GitHub x86-64 runner with AVX2, steps 2 and 3 together took `full` seed 1 from 2.71 s to 1.25 s. That is a slow shared machine, so the ratio matters, not the time. Measured on x86, the gain is much larger than the Mac suggested.

Result: to be filled in from the board run.

## Step 4: lookup without branches

Expected: lookup was still the largest phase. Whether a token is a hit, and whether it is new to the block, are both close to coin flips for the branch predictor, and checking for anchors read a second table.

Measured before pushing, on the Apple M5: CPU time fell from 1.45 s to 1.29 s, and wall time to 0.16 s. On the GitHub x86-64 runner it made no visible difference (1.25 s to 1.23 s), so the board run decides whether this step is worth keeping.

Result: to be filled in from the board run.

The change also caused a bug that only the fuzz test found. Writing `touched` unconditionally means every token writes one entry past the count, so a block containing every term ran off the end of the array. Real data almost never does this, but a tiny fuzz corpus does on its second round.

## Experiments still to try on the board

- Evaluating dense queries on every block from a plain list, and removing the branch on each operator. On the Mac neither changed CPU time beyond noise, at any threshold from 6 to 64 on seeds 2 and 3, so they are out of the code for now. The M5's large caches could hide a gain the i5 would show, so they get one board run each before being dropped.
