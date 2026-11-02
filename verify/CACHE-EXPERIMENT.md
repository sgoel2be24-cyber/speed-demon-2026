# Smaller term-table candidate

**Read from code:** the entry uses twelve workers again and changes the final
term-table capacity factor from 12 to 6. For full generated queries this is
expected to halve the slot array from 4 MiB to 2 MiB; exact capacity is rounded
to a power of two. Probe/key/long-token comparisons and the oversized-shard
correctness guard remain unchanged.

**Measured (historical runner evidence):** the handoff reports a half-size table
was 12.8% slower on a large-cache runner. That result is retained, not presented
as a favourable benchmark. No scoring-box speed or points are known for this
candidate.

**Hypothesis:** a smaller shared table could reduce cache pressure with twelve
scan workers on the scoring CPU's 24 MiB L3. Correctness and exact-image PGO
holdouts must pass before requesting any board test. The user's current approval
is limited to the six-worker candidate and restoration; it does not cover this
candidate. The goal of 82 points has not been demonstrated.

Source: local handoff section 7, and `track-b/src/terms.rs::renumber`.
