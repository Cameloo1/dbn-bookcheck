# Test coverage

Coverage was measured on 2026-07-30 with Rust 1.88.0 and `cargo-llvm-cov` 0.8.7:

```sh
CARGO_TARGET_DIR=/tmp/dbn-es-bench-coverage-target \
PROPTEST_CASES=256 \
cargo +1.88.0 llvm-cov --workspace --all-targets --locked --summary-only
```

| Scope | Line coverage |
| --- | ---: |
| `dbn-es-core/src/decode.rs` | 87.23% |
| `dbn-es-core/src/order_book.rs` | 85.94% |
| `dbn-es-core/src/sweep.rs` | 80.86% |
| Core crate, weighted | 85.07% |
| Entire workspace | 53.05% |

The workspace total includes the paid-network acquisition path, the full live-data benchmark runner, and the N-API export layer. Those paths are exercised by bounded acquisition verification, the committed benchmark procedure, and Node/Rust parity tests respectively, but those external processes do not contribute to Rust source-based coverage. No coverage threshold is claimed for the workspace total.
