#!/usr/bin/env sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

FULL=0
if [ "${1-}" = "--full" ]; then
  FULL=1
  shift
fi
if [ "$#" -ne 0 ]; then
  echo "usage: scripts/verify.sh [--full]" >&2
  exit 2
fi

echo "==> Rust quality gate"
cargo fmt --all --check
cargo check --workspace --all-targets --locked
PROPTEST_CASES=256 cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked

SAMPLE_DIR="$ROOT/data/sample"
SAMPLE_MANIFEST="$SAMPLE_DIR/manifest.json"

echo "==> Deterministic synthetic fixture"
cargo run --release --locked -p dbn-es-bench --bin dbn-es-bench -- data sample --output-dir "$SAMPLE_DIR"
cargo run --release --locked -p dbn-es-bench --bin dbn-es-bench -- decode stats --manifest "$SAMPLE_MANIFEST" --output "$SAMPLE_DIR/decode-stats.json"
cargo run --release --locked -p dbn-es-bench --bin dbn-es-bench -- analyze validate --manifest "$SAMPLE_MANIFEST" --output "$SAMPLE_DIR/book-validation.md" --json-output "$SAMPLE_DIR/book-validation.json"
cargo run --release --locked -p dbn-es-bench --bin dbn-es-bench -- analyze sweeps --manifest "$SAMPLE_MANIFEST" --config "$ROOT/config/sweep.json" --output "$SAMPLE_DIR/sweeps.jsonl" --summary "$SAMPLE_DIR/sweep-summary.json" --report "$SAMPLE_DIR/sweep-detection.md"

if [ "$FULL" -eq 1 ]; then
  if [ ! -f "$ROOT/data/manifest.json" ] || [ ! -f "$ROOT/config/live-session.json" ]; then
    echo "full verification requires operator-restored data/manifest.json, config/live-session.json, and the referenced DBN files" >&2
    exit 3
  fi
  echo "==> Verified live corpus and promoted reports"
  cargo run --release --locked -p dbn-es-bench --bin dbn-es-bench -- data verify --config "$ROOT/config/live-session.json"
  cargo run --release --locked -p dbn-es-bench --bin dbn-es-bench -- decode stats --manifest "$ROOT/data/manifest.json" --output "$ROOT/data/decode-stats.json"
  cargo run --release --locked -p dbn-es-bench --bin dbn-es-bench -- analyze validate --manifest "$ROOT/data/manifest.json" --output "$ROOT/docs/book-validation.md" --json-output "$ROOT/data/book-validation.json"
  cargo run --release --locked -p dbn-es-bench --bin dbn-es-bench -- analyze sweeps --manifest "$ROOT/data/manifest.json" --config "$ROOT/config/sweep.json" --output "$ROOT/out/sweeps.jsonl" --summary "$ROOT/data/sweep-summary.json" --report "$ROOT/docs/sweep-detection.md"
  cargo run --release --locked -p dbn-es-bench --bin dbn-es-benchmark -- run --manifest "$ROOT/data/manifest.json" --uncompressed-dir "$ROOT/data/uncompressed" --output "$ROOT/bench/results.json"
fi

echo "==> Generated public reports"
if [ -f "$ROOT/bench/results.json" ] &&
  [ -f "$ROOT/bench/machine.json" ] &&
  [ -f "$ROOT/config/live-session.json" ] &&
  [ -f "$ROOT/evidence/public/acquisition-summary.json" ] &&
  [ -f "$ROOT/evidence/public/book-validation-summary.json" ] &&
  [ -f "$ROOT/evidence/public/parity-summary.json" ] &&
  [ -f "$ROOT/evidence/public/sweep-summary.json" ]; then
  node scripts/generate-bench-report.mjs bench/results.json bench/results.md
  node scripts/generate-readme.mjs
  node scripts/generate-presentation.mjs
  node scripts/generate-public-report-data.mjs --check
else
  echo "local evidence inputs are intentionally absent; validating promoted public reports"
fi
node scripts/validate-results-html.mjs docs/results.html
node scripts/validate-public-report-data.mjs
node scripts/audit-repository.mjs

echo "==> Static portfolio"
(
  cd "$ROOT/web"
  if [ -f package-lock.json ]; then
    npm ci
  else
    npm install --no-package-lock
  fi
  npm run verify
)
node scripts/audit-web-output.mjs web/dist

echo "==> Bounded synthetic demo"
scripts/demo.sh

echo "==> Node/TypeScript package"
(
  cd "$ROOT/node"
  if [ -f package-lock.json ]; then
    npm ci
  else
    npm install --no-package-lock
  fi
  npm run build:native
  npm run typecheck
  npm run example -- "$SAMPLE_MANIFEST"
  npm run pack:check
  if [ "$FULL" -eq 1 ]; then
    npm run parity -- "$ROOT/data/manifest.json"
  fi
)

echo "verification passed (sample provenance: synthetic; full live evidence: $FULL)"
