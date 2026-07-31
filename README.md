# dbn-es-bench

`dbn-es-bench` is a Rust/Node toolkit for streaming Databento DBN files, reconstructing ES top-of-book, detecting transparent liquidity-sweep events, and benchmarking decode throughput.

## Results first

On a verified live `GLBX.MDP3` session, reconstructed MBO top-of-book matched aligned exchange MBP-10 price and size at **100.000000%** across **23,961,616 updates**. The live compressed MBO decode path sustained a **3,454,156 messages/s median** (184.47 decoded MiB/s, 289.51 ns/message).

Machine: AMD Ryzen 7 7700X 8-Core Processor (8 cores/16 threads), 32 GiB DDR5-6000, NVMe SSD; Ubuntu on WSL 2 6.6.87.2-microsoft-standard-WSL2, rustc 1.96.0. The host's otherwise-idle state was not verified, so these are observed results, not a cross-platform guarantee.

## Quickstart

Clone the repository, then run the clean synthetic path:

```sh
git clone https://github.com/Cameloo1/dbn-bookcheck.git
cd dbn-bookcheck
cargo build --release --locked
cargo run --release --locked -p dbn-es-bench --bin dbn-es-bench -- data sample --output-dir data/sample
cargo run --release --locked -p dbn-es-bench --bin dbn-es-bench -- decode stats --manifest data/sample/manifest.json --output data/sample/decode-stats.json
```

The generated files are deterministic, spec-valid, and visibly marked `synthetic`; they are test/demo inputs, not market evidence.

## Interactive case study

The static portfolio in `web/` presents the promoted aggregate evidence and
deterministic synthetic teaching sequences. It has no backend and reads no
purchased DBN payload at runtime.

```sh
node scripts/generate-public-report-data.mjs --check
npm --prefix web install --no-package-lock
npm --prefix web run verify
```

The checked-in GitHub Pages workflow builds and audits the static artifact.
Deployment remains a separate repository-owner action.

## Architecture

```text
DBN / DBN.ZST file
        |
        v
dbn-es-core typed, fallible streaming decoder
        |
        +--> sequence/timestamp diagnostics
        +--> per-instrument MBO order books --> MBP-10 validation
        +--> pre-event book + trades --------> sweep detector
        |
        v
dbn-es-bench CLI/benchmark       dbn-es-node N-API --> TypeScript package
```

- `dbn-es-core` owns strict DBN boundaries, zero-copy borrowed record streams, snapshot-gated order-book state, and the four-parameter sweep state machine.
- `dbn-es-bench` owns spend-gated acquisition, manifests, validation reports, deterministic sample generation, and isolated benchmark orchestration.
- `dbn-es-node` converts owned values only at the N-API boundary and preserves 64-bit timestamps, prices, sizes, and counts as JavaScript `bigint` where required.
- `node` contains the generated loader, TypeScript declarations, example, parity test, and per-target package metadata.

## Benchmark

Each row uses one discarded warmup and five measured fresh-process samples. Rates include file I/O, decompression where applicable, DBN parsing, traversal, thread startup, and joins. Parallel rows are aggregate rates over independent identical streams; they do not claim that one Zstd stream is splittable. Full methodology and capability disclosures are in [`bench/results.md`](bench/results.md).

| Schema | Compression | Access | Concurrency | Runs | Msg/s median | Msg/s p95 | Wire MiB/s | Decoded MiB/s | ns/msg | Peak RSS MiB median/max |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| mbo | zstd | streaming | single thread (1) | 5 | 3454156 | 3515012 | 53.42 | 184.47 | 289.51 | 5.63/5.63 |
| mbo | zstd | streaming | parallel independent streams (4) | 5 | 11079259 | 11225820 | 171.35 | 591.70 | 90.26 | 12.63/12.75 |
| mbo | zstd | fully buffered input | single thread (1) | 5 | 8659401 | 9138992 | 133.93 | 462.46 | 115.48 | 522.50/522.63 |
| mbo | zstd | fully buffered input | parallel independent streams (4) | 5 | 21745886 | 22265186 | 336.32 | 1161.36 | 45.99 | 2081.00/2081.00 |
| mbo | none | streaming | single thread (1) | 5 | 3448848 | 3472830 | 184.19 | 184.19 | 289.95 | 3.13/3.25 |
| mbo | none | streaming | parallel independent streams (4) | 5 | 7522091 | 7892918 | 401.72 | 401.72 | 132.94 | 3.13/3.25 |
| mbo | none | fully buffered input | single thread (1) | 5 | 4063340 | 4127925 | 217.01 | 217.01 | 246.10 | 1788.63/1788.75 |
| mbo | none | fully buffered input | parallel independent streams (4) | 5 | 8658300 | 8986890 | 462.40 | 462.40 | 115.50 | 7146.00/7146.13 |
| mbp-10 | zstd | streaming | single thread (1) | 5 | 1974560 | 2001871 | 48.54 | 692.98 | 506.44 | 5.63/5.63 |
| mbp-10 | zstd | streaming | parallel independent streams (4) | 5 | 6892871 | 6970689 | 169.43 | 2419.07 | 145.08 | 12.63/12.75 |
| mbp-10 | zstd | fully buffered input | single thread (1) | 5 | 4523178 | 4591464 | 111.18 | 1587.42 | 221.08 | 599.63/599.75 |
| mbp-10 | zstd | fully buffered input | parallel independent streams (4) | 5 | 12425213 | 12705840 | 305.42 | 4360.66 | 80.48 | 2389.00/2389.00 |
| mbp-10 | none | streaming | single thread (1) | 5 | 856202 | 867531 | 300.49 | 300.49 | 1167.95 | 3.13/3.25 |
| mbp-10 | none | streaming | parallel independent streams (4) | 5 | 1548176 | 1585437 | 543.34 | 543.34 | 645.92 | 3.13/3.25 |
| mbp-10 | none | fully buffered input | single thread (1) | 5 | 384387 | 403412 | 134.90 | 134.90 | 2601.54 | 8485.38/8485.50 |
| ohlcv-1m | zstd | streaming | single thread (1) | 5 | 666225 | 707401 | 12.44 | 35.75 | 1500.99 | 3.25/3.38 |
| ohlcv-1m | zstd | streaming | parallel independent streams (4) | 5 | 2320855 | 2360296 | 43.32 | 124.52 | 430.88 | 3.75/3.88 |
| ohlcv-1m | zstd | fully buffered input | single thread (1) | 5 | 803967 | 836170 | 15.01 | 43.14 | 1243.83 | 3.38/3.50 |
| ohlcv-1m | zstd | fully buffered input | parallel independent streams (4) | 5 | 2325576 | 2643774 | 43.41 | 124.78 | 430.00 | 3.63/3.75 |
| ohlcv-1m | none | streaming | single thread (1) | 5 | 646206 | 656448 | 34.67 | 34.67 | 1547.49 | 3.13/3.13 |
| ohlcv-1m | none | streaming | parallel independent streams (4) | 5 | 2026961 | 2124751 | 108.76 | 108.76 | 493.35 | 3.25/3.25 |
| ohlcv-1m | none | fully buffered input | single thread (1) | 5 | 685605 | 806061 | 36.79 | 36.79 | 1458.57 | 3.13/3.25 |
| ohlcv-1m | none | fully buffered input | parallel independent streams (4) | 5 | 2100577 | 2279091 | 112.71 | 112.71 | 476.06 | 3.63/3.63 |
| trades | zstd | streaming | single thread (1) | 5 | 3648983 | 3834452 | 53.96 | 167.04 | 274.05 | 5.50/5.50 |
| trades | zstd | streaming | parallel independent streams (4) | 5 | 12291806 | 13211038 | 181.77 | 562.68 | 81.36 | 12.63/12.63 |
| trades | zstd | fully buffered input | single thread (1) | 5 | 9678054 | 9736047 | 143.12 | 443.03 | 103.33 | 25.88/25.88 |
| trades | zstd | fully buffered input | parallel independent streams (4) | 5 | 25561446 | 26937188 | 378.01 | 1170.12 | 39.12 | 94.00/94.13 |
| trades | none | streaming | single thread (1) | 5 | 4095092 | 4154890 | 187.46 | 187.46 | 244.19 | 3.13/3.13 |
| trades | none | streaming | parallel independent streams (4) | 5 | 9476468 | 10491629 | 433.80 | 433.80 | 105.52 | 3.13/3.13 |
| trades | none | fully buffered input | single thread (1) | 5 | 5149157 | 5363227 | 235.71 | 235.71 | 194.21 | 66.13/66.13 |
| trades | none | fully buffered input | parallel independent streams (4) | 5 | 11668184 | 12128783 | 534.13 | 534.13 | 85.70 | 255.50/255.50 |

## Book validation

The live validation scanned 33,438,049 MBO records and 24,171,039 MBP-10 records. Reconstruction stayed withheld until a complete snapshot/clear boundary. After that boundary, all 23,961,616 MBP-10 updates aligned by timestamp, publisher, instrument, sequence, action, price, and size, and all matched reconstructed bid/ask price and aggregate size. See [`docs/book-validation.md`](docs/book-validation.md) for the denominator, unmatched MBO observations, and failure-mode analysis.

## Sweep detector

The committed detector uses a 50-trade rolling high/low, a 4-tick penetration, a 5,000 ms reversion window, and an ES tick size of 250,000,000 DBN fixed-price units. The verified live scan emitted 11 events. This is a transparent heuristic, not a trading signal.

Each JSONL event contains:

- publisher/instrument identity;
- emission, threshold-crossing, and reversion timestamps;
- `above_high` or `below_low` direction;
- swept level, maximum displacement in ticks, and duration;
- conservatively attributed visible resting size.

The definition and attribution boundary are in [`docs/sweep-detection.md`](docs/sweep-detection.md).

## Limitations

- The headline benchmark measures DBN decode/traversal, not order-book reconstruction, detector work, Node call overhead, networking, or end-to-end trading latency. Page cache is warm; safe cold-cache eviction and allocation instrumentation were unavailable. One 35.58 GB parallel buffered configuration is explicitly unsupported by the 16 GiB memory gate.
- The live session and generated evidence are intentionally gitignored. Reproduction of live claims requires restoring the checksum-verified corpus described by `data/manifest.json`; clean checkout CI and demos use the labeled synthetic generator.
- Validation covers top-of-book price and aggregate size for one ES continuous-symbol session after a complete MBO baseline. It is not proof for every venue, schema version, depth level, instrument, or data-quality condition.
- The book invalidates on bad-book flags, rejects inconsistent/out-of-order transitions, and waits for a new complete baseline. It does not guess through missing recovery data.
- Sweep events depend on four fixed parameters and visible pre-event book state. `resting_size_consumed` does not estimate hidden liquidity or total multi-level execution size.
- Acquisition safety and replay-state logic have unit and one successful live-session evidence, but provider timeouts and interrupted-download recovery were not fault-injected against the paid API.
- The results page is checked for deterministic self-containment, accessible inline SVG, and tabular fallbacks; this run did not execute a cross-browser visual compatibility matrix.
- Native packages are prepared for four common targets but are not published. Until hosted cross-platform CI and release provenance exist, consumers build the addon locally.

## Reproduce

Fast clean-checkout verification (synthetic provenance, no paid request):

```sh
./scripts/verify.sh
```

PowerShell equivalent on Windows hosts that permit locally compiled Rust executables:

```powershell
.\scripts\verify.ps1
```

After restoring the verified live `data/manifest.json` corpus, regenerate every promoted live number and report, including the full benchmark:

```sh
./scripts/verify.sh --full
```

Neither verification mode acquires data or spends money. Acquisition is a separate, quote-first command protected by the committed $10 cap and replay ledger. Generated payloads, raw logs, native binaries, and coverage profiles stay outside Git.

## License and attribution

Licensed under [Apache 2.0](LICENSE). Databento defines the [DBN format](https://databento.com/docs/standards-and-conventions/databento-binary-encoding) and maintains the upstream Rust crates used for decoding and historical access.
