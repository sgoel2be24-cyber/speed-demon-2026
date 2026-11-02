# Track B: Index

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/b-index.md).

## Approach

This solution builds no index. The queries are readable from the start, so it reads them first, then answers all of them during one pass over the corpus on 12 threads. Nothing about the corpus is kept beyond the current 256 documents, and the file itself is memory-mapped, so peak memory stays under the 44 MB where the memory score stops improving.

**BOOL queries** run on blocks of 256 documents. Each thread keeps a 256-bit word per query term, where bit `i` says whether document `i` of the block contains the term. A query is then evaluated on all 256 documents at once: `AND`, `OR` and `NOT` become `&`, `|` and `& !`, applied strictly left to right as the spec defines them.

Evaluating all 31,000 queries on every block would be far too slow, so each query has a *cover*: a set of its terms such that any matching document must contain at least one of them. A query is evaluated only in blocks where a cover term occurs. The rules follow the left-to-right evaluation, with R the result so far: a match of `R AND t` contains `t` and is in R, so either side's cover works; a match of `R OR t` needs both; a match of `R NOT t` needs only R's. The cover decides when to evaluate, never what the answer is.

**PHRASE queries** need no stored positions. Each document's tokens become an array of term ids, used only for that document. Each phrase is anchored on its rarest-looking term, and at each occurrence of that term the neighbouring ids are compared with the phrase. A phrase cannot cross into another document, and punctuation between terms is invisible because separators are not in the array.

How often a term appears in `queries.txt` turned out to be a good stand-in for how rare it is in the corpus, so choosing covers and anchors needs no corpus statistics at all.

Scattered memory reads turned out to be the main cost on the scoring CPU, so the layout is built around avoiding them. Terms used at least four times in the queries are dense: their words sit in a small array that stays in cache. Each query with dense terms is folded to those terms alone, which is its exact meaning in any block that holds none of its rare terms, and that short form runs in every block. Only when a rare term occurs does the full query run instead, for that block. Rare terms take a place in a small arena only while they are in the block. The term table is larger than it needs to be, on huge pages where the system allows, because a hash collision turned out to cost more than a cache miss.

Each thread keeps running totals for its own part of the corpus. The parts are in document order, so joining them gives each query's count, sum and ten smallest ids.

The binary is built with profile guidance, trained on the organisers' generator at build time on a seed other than the public one.

## Dependencies

None.

## With another 24 hours

- **Batched lookups.** Lookup and setting term words take about 41% of CPU, mostly waiting on cache misses one token at a time. Handling several tokens at once with AVX2 is the largest chance left.
- **Table size on the scoring box.** The larger table won on test machines with far more L3 cache. With 12 threads sharing 24 MB, a smaller one may do better, but this needs several board runs per size to see through the noise.
- **Fewer lines.** The entry grew to about 1,000 lines. Some of that can be simplified without losing speed.
- **Hardware counters on the scoring machine.** The largest open question is why the leaders use about a quarter of this CPU time. Cache-miss counts per token would answer it directly, where this work had to infer them from timing.
