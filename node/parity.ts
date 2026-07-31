import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  MboSweepDetector,
  decodeStats,
  type JsSweepEvent,
} from "./native.js";
import { json, loadMboEntry, loadSweepConfig, workspaceRoot } from "./support.js";

interface RustDecodeFile {
  path: string;
  schema: string | null;
  record_count: number;
  first_timestamp_ns: string | null;
  last_timestamp_ns: string | null;
  min_timestamp_ns: string | null;
  max_timestamp_ns: string | null;
  timestamp_regression_count: number;
  record_counts_by_type: Record<string, number>;
  sequence: {
    records_observed: number;
    forward_gap_count: number;
    missing_sequence_count: number;
    duplicate_count: number;
    regression_count: number;
  } | null;
}

interface RustDecodeReport {
  files: RustDecodeFile[];
}

interface RustSweepEvent {
  instrument: { publisher_id: number; instrument_id: number };
  timestamp_ns: string;
  sweep_timestamp_ns: string;
  reversion_timestamp_ns: string;
  direction: string;
  swept_level: string;
  displacement_ticks: string;
  duration_ns: string;
  resting_size_consumed: string;
}

const integerKeys = [
  "first_timestamp_ns",
  "last_timestamp_ns",
  "min_timestamp_ns",
  "max_timestamp_ns",
  "timestamp_ns",
  "sweep_timestamp_ns",
  "reversion_timestamp_ns",
  "swept_level",
  "displacement_ticks",
  "duration_ns",
  "resting_size_consumed",
];
const integerPattern = new RegExp(
  `(\"(?:${integerKeys.join("|")})\"\\s*:\\s*)(-?\\d+)`,
  "g",
);

function parseLossless<T>(text: string): T {
  return JSON.parse(text.replace(integerPattern, '$1"$2"')) as T;
}

function asString(value: bigint | number | null | undefined): string | null {
  return value === null || value === undefined ? null : value.toString();
}

function normalizedEvent(event: JsSweepEvent): Omit<RustSweepEvent, "instrument"> & {
  instrument: RustSweepEvent["instrument"];
} {
  return {
    instrument: {
      publisher_id: event.publisherId,
      instrument_id: event.instrumentId,
    },
    timestamp_ns: event.timestampNs.toString(),
    sweep_timestamp_ns: event.sweepTimestampNs.toString(),
    reversion_timestamp_ns: event.reversionTimestampNs.toString(),
    direction: event.direction,
    swept_level: event.sweptLevel.toString(),
    displacement_ticks: event.displacementTicks.toString(),
    duration_ns: event.durationNs.toString(),
    resting_size_consumed: event.restingSizeConsumed.toString(),
  };
}

const entry = loadMboEntry(process.argv[2]);
const stats = decodeStats(entry.path);
const rustDecodePath = resolve(workspaceRoot, "data", "decode-stats.json");
const rustDecode = parseLossless<RustDecodeReport>(readFileSync(rustDecodePath, "utf8"));
const expectedStats = rustDecode.files.find(
  (file) => resolve(workspaceRoot, file.path) === entry.path,
);
assert.ok(expectedStats, `Rust decode report has no entry for ${entry.path}`);
assert.equal(stats.schema, expectedStats.schema);
assert.equal(stats.recordCount.toString(), expectedStats.record_count.toString());
assert.equal(asString(stats.firstTimestampNs), expectedStats.first_timestamp_ns);
assert.equal(asString(stats.lastTimestampNs), expectedStats.last_timestamp_ns);
assert.equal(asString(stats.minTimestampNs), expectedStats.min_timestamp_ns);
assert.equal(asString(stats.maxTimestampNs), expectedStats.max_timestamp_ns);
assert.equal(
  stats.timestampRegressionCount.toString(),
  expectedStats.timestamp_regression_count.toString(),
);
assert.deepEqual(
  Object.fromEntries(
    stats.recordCountsByType.map((item) => [item.recordType, Number(item.count)]),
  ),
  expectedStats.record_counts_by_type,
);
if (expectedStats.sequence === null) {
  assert.equal(stats.sequence, null);
} else {
  assert.ok(stats.sequence);
  for (const [actual, expected] of [
    [stats.sequence.recordsObserved, expectedStats.sequence.records_observed],
    [stats.sequence.forwardGapCount, expectedStats.sequence.forward_gap_count],
    [stats.sequence.missingSequenceCount, expectedStats.sequence.missing_sequence_count],
    [stats.sequence.duplicateCount, expectedStats.sequence.duplicate_count],
    [stats.sequence.regressionCount, expectedStats.sequence.regression_count],
  ] as const) {
    assert.equal(actual.toString(), expected.toString());
  }
}

const config = loadSweepConfig();
const detector = new MboSweepDetector(entry.path, config);
const actualSweeps: ReturnType<typeof normalizedEvent>[] = [];
for (;;) {
  const event = detector.nextSweep();
  if (event === null) {
    break;
  }
  actualSweeps.push(normalizedEvent(event));
}
const expectedSweeps = readFileSync(resolve(workspaceRoot, "out", "sweeps.jsonl"), "utf8")
  .trim()
  .split(/\r?\n/u)
  .filter(Boolean)
  .map((line) => parseLossless<RustSweepEvent>(line));
assert.deepEqual(actualSweeps, expectedSweeps);

console.log(
  `Node/Rust parity passed: ${json({ recordCount: stats.recordCount, recordsScanned: detector.recordsScanned, sweepCount: actualSweeps.length })}`,
);
