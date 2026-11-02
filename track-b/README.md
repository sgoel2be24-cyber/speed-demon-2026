# Track B: Index

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/b-index.md).

## Approach

This solution builds no index. The queries are readable from the start, so it reads them first, then answers all of them during one pass over the corpus. The current container candidate explicitly uses six scan workers; the command-line binary also accepts a worker count. Nothing about the corpus is kept beyond the current 256 documents per worker, and the file itself is memory-mapped. The historical board version used twelve workers and peaked below the 44 MB memory-scoring cap; the new candidate's board memory and time are not yet measured.

**BOOL queries** run on blocks of 256 documents. Each thread keeps a 256-bit word per query term, where bit `i` says whether document `i` of the block contains the term. A query is then evaluated on all 256 documents at once: `AND`, `OR` and `NOT` become `&`, `|` and `& !`, applied strictly left to right as the spec defines them.

Evaluating all 31,000 queries on every block would be far too slow, so each query has a *cover*: a set of its terms such that any matching document must contain at least one of them. A query is evaluated only in blocks where a cover term occurs. The rules follow the left-to-right evaluation, with R the result so far: a match of `R AND t` contains `t` and is in R, so either side's cover works; a match of `R OR t` needs both; a match of `R NOT t` needs only R's. The cover decides when to evaluate, never what the answer is.

**PHRASE queries** need no stored positions. Each document's tokens become an array of term ids, used only for that document. Each phrase is anchored on its rarest-looking term, and at each occurrence of that term the neighbouring ids are compared with the phrase. A phrase cannot cross into another document, and punctuation between terms is invisible because separators are not in the array.

How often a term appears in `queries.txt` turned out to be a good stand-in for how rare it is in the corpus, so choosing covers and anchors needs no corpus statistics at all.

The layout reduces scattered reads, a useful target in x86 runner profiles and A/B experiments. Frequently used BOOL terms get dense ids and contiguous 256-bit words. Compilation treats every rare term as absent and builds a folded form when that leaves an expression with dense terms. A query such as `a AND rare` has no such form, even if `a` is dense. A nonempty form runs in each block unless a watched rare term selects the full query instead. Queries without a form run only when triggered, and each BOOL query is evaluated at most once per block. Rare terms take a place in a small arena only while they are in the block. A larger term table reduced probing in runner measurements; huge pages reduce address-translation pressure where supported. Neither observation establishes the cause of live-board timing noise.

Each thread keeps running totals for its own part of the corpus. The parts are in document order, so joining them gives each query's count, sum and ten smallest ids. Byte-balanced parts are also bounded to 65,535 documents each, because local ids and counts use 16 bits. A part with too many short documents is split further; unusually skewed text can therefore use more workers than the requested count.

First matches are stored by rank across queries, so the first id for each query occupies a contiguous region. The hash selects a home using the first eight token bytes; full 16-byte key comparisons, and full comparisons for longer tokens, still decide whether a token matches. These layout/hash choices cannot turn a collision into a matching term.

The binary is built with profile guidance, trained on the organisers' generator at build time on a seed other than the public one.

## Dependencies

None.

## Verification

The `verify` workflow builds from inside `track-b/` and checks that AVX2 is enabled, then compares every output with the organisers' reference on full seeds 1–3, sample seeds 1–9 and tiny seeds 1–50. It also checks the actual PGO Docker image on full seeds 2 and 3. Differential fuzz includes exact 256- and 512-document boundaries, and deterministic regressions cover the local-id limit and skewed shards. Tests stay outside the scored directory. See the [verification audit](../verify/AUDIT.md) for measured results, limitations and commands; the presence of a workflow alone is not a passing result.

## With another 24 hours

- **Batched lookups (hypothesis).** The older step-7 profile put lookup and word setting at about 41% of CPU. The dense-term patch changed the workload, so a fresh profile is needed before claiming the same share or predicting a gain from handling several tokens at once with AVX2.
- **Table size on the scoring box (hypothesis).** The larger table won on test machines with far more L3 cache. With 12 threads sharing 24 MB, a smaller one may do better, but this needs several board runs per size to see through the noise.
- **Fewer lines.** The entry grew to about 1,000 lines. Some of that can be simplified without losing speed.
- **Hardware counters on the scoring machine (hypothesis).** The cause of the live-board timing spread is unresolved. Cache-miss and scheduling measurements could test the shared-cache and shard-imbalance hypotheses; the current timing observations cannot distinguish them.
