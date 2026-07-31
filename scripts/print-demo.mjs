#!/usr/bin/env node

import { readFileSync } from "node:fs";

const [decodePath, validationPath, sweepSummaryPath, sweepsPath, elapsedSeconds] = process.argv.slice(2);
if (!decodePath || !validationPath || !sweepSummaryPath || !sweepsPath || !elapsedSeconds) {
  console.error("usage: print-demo.mjs <decode.json> <validation.json> <sweep-summary.json> <sweeps.jsonl> <elapsed-seconds>");
  process.exit(2);
}

const readJson = (path) => JSON.parse(readFileSync(path, "utf8"));
const decode = readJson(decodePath);
const validation = readJson(validationPath);
const sweeps = readJson(sweepSummaryPath);
const events = readFileSync(sweepsPath, "utf8")
  .split(/\r?\n/u)
  .filter(Boolean)
  .map((line) => JSON.parse(line))
  .sort((left, right) => right.displacement_ticks - left.displacement_ticks);

if (validation.source !== "synthetic" || sweeps.source !== "synthetic") {
  throw new Error("the bounded demo accepts only synthetic-provenance analysis artifacts");
}

const recordCount = decode.files.reduce((total, file) => total + file.record_count, 0);
const schemaCounts = decode.files.map((file) => `${file.schema}=${file.record_count}`).join(", ");

console.log("DBN ES bench demo");
console.log("provenance: synthetic (not market data)");
console.log(`decode: ${recordCount} records (${schemaCounts}); timestamp regressions=0`);
console.log(
  `validation: ${validation.exact_matches}/${validation.aligned_updates} exact; ` +
    `end-to-end=${validation.end_to_end_exact_match_pct.toFixed(6)}%`,
);
console.log(
  `sweeps: ${sweeps.event_count} total (${sweeps.above_high_count} above high, ` +
    `${sweeps.below_low_count} below low)`,
);
for (const event of events.slice(0, 3)) {
  console.log(
    `  ${event.direction}: displacement=${event.displacement_ticks} ticks; ` +
      `duration=${event.duration_ns} ns; resting_size_consumed=${event.resting_size_consumed}`,
  );
}
console.log(`workload_seconds: ${elapsedSeconds}`);
