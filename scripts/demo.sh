#!/usr/bin/env sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

if [ "$#" -ne 0 ]; then
  echo "usage: scripts/demo.sh" >&2
  exit 2
fi

BIN=${DBN_ES_BENCH_BIN:-"$ROOT/target/release/dbn-es-bench"}
if [ ! -x "$BIN" ]; then
  echo "==> Building the release CLI (one-time setup; excluded from the demo workload timer)"
  cargo build --release --locked -p dbn-es-bench --bin dbn-es-bench
fi

TEMP_BASE=${TMPDIR:-/tmp}
WORK=$(mktemp -d "$TEMP_BASE/dbn-es-demo.XXXXXX")
KEEP=${DBN_ES_KEEP_DEMO:-0}
cleanup() {
  status=$?
  if [ "$status" -ne 0 ] || [ "$KEEP" = "1" ]; then
    echo "demo workspace retained for inspection: $WORK" >&2
  else
    case "$WORK" in
      "$TEMP_BASE"/dbn-es-demo.*) rm -rf -- "$WORK" ;;
      *) echo "refusing to remove unexpected demo workspace: $WORK" >&2 ;;
    esac
  fi
  exit "$status"
}
trap cleanup EXIT HUP INT TERM

START=$(date +%s)
echo "==> Generating deterministic four-schema fixture"
"$BIN" data sample --output-dir "$WORK" >/dev/null
echo "==> Streaming decode and integrity scan"
"$BIN" decode stats --manifest "$WORK/manifest.json" --output "$WORK/decode-stats.json" >/dev/null
echo "==> Reconstructing top of book and validating against MBP-10"
"$BIN" analyze validate --manifest "$WORK/manifest.json" --output "$WORK/book-validation.md" --json-output "$WORK/book-validation.json" >/dev/null
echo "==> Detecting and ranking bounded sweep/reversion events"
"$BIN" analyze sweeps --manifest "$WORK/manifest.json" --config "$ROOT/config/sweep.json" --output "$WORK/sweeps.jsonl" --summary "$WORK/sweep-summary.json" --report "$WORK/sweep-detection.md" >/dev/null
END=$(date +%s)
ELAPSED=$((END - START))

node "$ROOT/scripts/print-demo.mjs" "$WORK/decode-stats.json" "$WORK/book-validation.json" "$WORK/sweep-summary.json" "$WORK/sweeps.jsonl" "$ELAPSED"
if [ "$ELAPSED" -ge 60 ]; then
  echo "demo workload exceeded the 60-second gate: ${ELAPSED}s" >&2
  exit 1
fi
