import {
  MboDecoder,
  MboSweepDetector,
  decodeStats,
  type JsSweepEvent,
} from "./native.js";
import { json, loadMboEntry, loadSweepConfig } from "./support.js";

const entry = loadMboEntry(process.argv[2]);
const stats = decodeStats(entry.path);
console.log(
  `decode stats: ${json({
    path: stats.path,
    schema: stats.schema,
    recordCount: stats.recordCount,
    firstTimestampNs: stats.firstTimestampNs,
    lastTimestampNs: stats.lastTimestampNs,
    timestampRegressionCount: stats.timestampRegressionCount,
    sequence: stats.sequence === undefined
      ? undefined
      : {
          forwardGapCount: stats.sequence.forwardGapCount,
          duplicateCount: stats.sequence.duplicateCount,
          regressionCount: stats.sequence.regressionCount,
          samplesTruncated: stats.sequence.samplesTruncated,
        },
  })}`,
);

const decoder = new MboDecoder(entry.path);
for (let index = 0; index < 3; index += 1) {
  const record = decoder.nextRecord();
  if (record === null) {
    break;
  }
  console.log(`record ${index + 1}: ${json(record)}`);
}

const config = loadSweepConfig();
const detector = new MboSweepDetector(entry.path, config);
const sweeps: JsSweepEvent[] = [];
for (;;) {
  const event = detector.nextSweep();
  if (event === null) {
    break;
  }
  sweeps.push(event);
  console.log(`sweep ${sweeps.length}: ${json(event)}`);
}

console.log(
  `summary: ${json({ recordsScanned: detector.recordsScanned, sweepCount: sweeps.length })}`,
);
