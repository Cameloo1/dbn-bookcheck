# Five-minute demo

This walkthrough uses the committed deterministic generator. Its provenance is `synthetic`; it does not download or impersonate live market data.

## 1. Build once

Prerequisites are Rust 1.88 or newer and Node.js 18 or newer. PowerShell is supported on Windows hosts that permit locally compiled Rust executables; Ubuntu on WSL 2 is the documented fallback when Windows Application Control blocks them.

```sh
cargo build --release --locked -p dbn-es-bench --bin dbn-es-bench
```

The first dependency compilation is machine- and cache-dependent. The sub-60-second gate applies to the bounded demo workload after this standard build.

## 2. Run the pipeline

```sh
scripts/demo.sh
```

The script creates a disposable workspace, generates all four schemas, streams them once, validates reconstructed top of book, runs the sweep heuristic, prints a compact result, and removes successful output. Set `DBN_ES_KEEP_DEMO=1` to retain evidence; failed runs are always retained.

Expected result (the final elapsed value varies and must remain below 60):

```text
DBN ES bench demo
provenance: synthetic (not market data)
decode: 162 records (mbo=55, mbp-10=54, trades=52, ohlcv-1m=1); timestamp regressions=0
validation: 54/54 exact; end-to-end=100.000000%
sweeps: 1 total (1 above high, 0 below low)
  above_high: displacement=4 ticks; duration=100000000 ns; resting_size_consumed=1
workload_seconds: <60
```

## 3. Inspect the production evidence

The demo proves the clean-checkout execution path. It is not the live benchmark. The promoted live evidence is in `bench/results.md`, `docs/book-validation.md`, and `docs/sweep-detection.md`; regenerate it only from a checksum-verified restored corpus with `scripts/verify.sh --full`.
