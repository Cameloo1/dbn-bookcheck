# DBN ES benchmark results

The headline measured path streamed the live compressed MBO session at **3454156 messages/s median** (184.47 decoded MiB/s, 289.51 ns/message). This is an observed result on the machine below, not a cross-platform guarantee.

## Reproduce

```sh
cargo run --release -p dbn-es-bench --bin dbn-es-benchmark -- run
node scripts/generate-bench-report.mjs
```

The raw benchmark JSON and machine capture are intentionally kept out of the public repository. A full local verification run regenerates this Markdown byte-identically from `bench/results.json` and the private machine capture; the sanitized, versioned public evidence is published at `web/public/data/report.v1.json`.

## Environment

- Generated: 2026-07-31T02:07:25.601533075Z
- Data: live
- CPU: AMD Ryzen 7 7700X 8-Core Processor (8 physical, 16 logical cores)
- Memory: 32 GiB DDR5-6000
- Storage: XG7000-2TB 2280 NVMe SSD
- Runtime: Ubuntu on WSL 2, kernel 6.6.87.2-microsoft-standard-WSL2, rustc 1.96.0 (ac68faa20 2026-05-25)
- Profile/cache: release, warm page cache; host idle status not verified

## Method

Each configuration runs in a fresh child process. One warmup is discarded and at least five measured runs are aggregated with median, nearest-rank p95, and population standard deviation. Timings include file reads, decompression, DBN parsing, record traversal, thread startup, and joins. Parallel rows decode independent identical streams and report aggregate throughput.

Rates in parallel rows are aggregate rates over independent identical streams. The same input is intentionally decoded once per worker; these rows do not claim one Zstd stream can be split.

## Capability matrix

| Capability | Status | Detail |
| --- | --- | --- |
| warm page cache | measured | Every configuration is warmed explicitly before recorded child runs. |
| cold page cache | unsupported | The managed WSL run has no safe, isolated OS page-cache eviction primitive; no cold numbers are fabricated. |
| peak resident set size | measured | Each child reads its own Linux /proc/self/status VmHWM value. |
| allocation count | not instrumentable | The forbid-unsafe workspace does not install a global allocator shim; results use null rather than zero. |
| parallel decode | measured | Parallel rows run independent streams, because one DBN/Zstd stream is sequential and is not falsely presented as splittable. |

## Measured configurations

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

Each row also records elapsed-time median/p95/population standard deviation, message-rate population standard deviation, byte counts, discarded warmups, and a null allocation count in the promoted public report.

## Unsupported configurations

| Schema | Compression | Access | Concurrency | Reason |
| --- | --- | --- | --- | --- |
| mbp-10 | none | fully buffered input | parallel independent streams | planned input buffers total 35579770848 bytes, above the configured 17179869184-byte safety gate |
