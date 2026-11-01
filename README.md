# Speed Demon 2026: Track B

Our entry for Track B (Index) of [Speed Demon 2026](https://speeddemon.dev). It answers 50,000 boolean and phrase queries over a 254 MB corpus in one pass, with no index.

| Folder | What it is |
|---|---|
| [`track-b/`](track-b) | The entry: Rust, no dependencies. [`README.md`](track-b/README.md) explains the approach, and [`PERF.md`](track-b/PERF.md) is the optimisation log with board run IDs. |
| [`verify/`](verify) | Checks against the organisers' reference: every level of data, plus a differential fuzz test. |
| [`.github/workflows/`](.github/workflows) | Runs those checks on x86-64 on every push, on seeds the board never uses. |

To build the entry and check it the way the board does, use `check.py`:

```
python3 check.py b --data <folder with docs.txt and queries.txt>
```

The code is under the MIT licence in `LICENSE`.
