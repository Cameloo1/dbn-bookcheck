# Technical summary

## What was built

This repository implements a bounded, streaming Rust pipeline for Databento Binary Encoding data: checksum-verified acquisition, schema-aware decoding, market-by-order reconstruction, exact top-of-book comparison against exchange MBP-10, a parameterized sweep/reversion heuristic, a process-isolated benchmark harness, and a Node-API binding. The core owns decoding and state transitions; the CLI owns files, acquisition gates, reports, and orchestration; the binding exposes streaming iterators without duplicating market logic.

## What was measured

The live evidence is one `GLBX.MDP3` `ES.v.0` session from 2025-04-03 22:00 UTC through 2025-04-04 21:00 UTC. Reconstruction scanned 33,438,049 MBO and 24,171,039 MBP-10 records. All 23,961,616 of 23,961,616 eligible exchange updates matched reconstructed bid/ask price and size (100.000000% end to end). The table below uses the comparable live Zstd, streaming, single-thread configuration; each value is the median of five measured fresh child processes after one discarded warmup.

| Schema | Records/run | Messages/s | Decoded MiB/s | Peak RSS MiB |
| --- | ---: | ---: | ---: | ---: |
| mbo | 33,438,049 | 3,454,156 | 184.47 | 5.63 |
| mbp-10 | 24,171,039 | 1,974,560 | 692.98 | 5.63 |
| trades | 1,378,526 | 3,648,983 | 167.04 | 5.50 |
| ohlcv-1m | 1,380 | 666,225 | 35.75 | 3.25 |

Host: AMD Ryzen 7 7700X 8-Core Processor; 8 physical/16 logical cores; 32 GiB DDR5-6000; SSD/NVMe; benchmark runtime Ubuntu on WSL 2.

## What was learned

The important correctness boundary was not applying every MBO record immediately. Reconstruction remains invalid until a complete clear/snapshot ending at the DBN event boundary; trade records compare the pre-event book, while state-changing events compare the final book. Unmatched eligible MBP-10 updates stay in the denominator. This produced 100.000000% exact agreement without treating pre-baseline or incomplete state as valid.

The performance result was similarly boundary-sensitive. A single compressed MBO stream sustained 3,454,156 messages/s (184.47 decoded MiB/s), while independent-stream parallelism is reported separately rather than implying one Zstd stream is splittable. One raw MBP-10 four-buffer case is explicitly unsupported because its planned 33.14 GiB allocation exceeds the 16 GiB safety gate.

## Production boundaries

The live dataset covers one instrument session, warm page cache, one CPU/OS combination, and top-of-book equality—not exchange connectivity, order routing, P&L, hidden liquidity, or trading performance. The 11 detected events (7 above prior highs, 4 below prior lows) are transparent heuristic output, not signals. Sample/CI evidence is deterministic synthetic data and is labeled as such. Exact regeneration starts at `scripts/verify.sh`; the full restored-corpus gate is `scripts/verify.sh --full`.
