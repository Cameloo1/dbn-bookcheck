#!/usr/bin/env node

import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { access, readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const relativeReportPath = process.argv[2] ?? "web/public/data/report.v1.json";

const allowedStatuses = new Set(["measured-live", "derived-live", "synthetic", "unsupported"]);
const intentionallyPrivateGenerationInputs = new Set([
  "bench/machine.json",
  "bench/results.json",
  "config/live-session.json",
  "evidence/public/acquisition-summary.json",
  "evidence/public/book-validation-summary.json",
  "evidence/public/parity-summary.json",
  "evidence/public/sweep-summary.json"
]);
const allowedUnits = new Set([
  "MiB/s",
  "USD",
  "bytes",
  "configurations",
  "events",
  "messages/s",
  "ns/message",
  "percent",
  "records"
]);

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function safeRepositoryPath(relativePath, context) {
  assert(typeof relativePath === "string" && relativePath.length > 0, `${context} must be a non-empty path`);
  assert(!path.isAbsolute(relativePath), `${context} must be repository-relative`);
  assert(!relativePath.split(/[\\/]/u).includes(".."), `${context} cannot traverse outside the repository`);
  return path.join(repoRoot, relativePath);
}

function resolveJsonPointer(document, pointer) {
  assert(typeof pointer === "string" && pointer.startsWith("/"), `invalid JSON pointer: ${pointer}`);
  return pointer
    .slice(1)
    .split("/")
    .map((token) => token.replaceAll("~1", "/").replaceAll("~0", "~"))
    .reduce((value, token) => {
      assert(value !== null && value !== undefined, `JSON pointer does not resolve: ${pointer}`);
      assert(Object.hasOwn(value, token), `JSON pointer does not resolve: ${pointer}`);
      return value[token];
    }, document);
}

function calculatedSourceValue(sourceValue, calculation) {
  if (!calculation) {
    return sourceValue;
  }
  if (calculation.kind === "array_length") {
    assert(Array.isArray(sourceValue), "array_length calculation requires an array source");
    return sourceValue.length;
  }
  throw new Error(`unknown claim calculation: ${calculation.kind}`);
}

function finiteNumbersOnly(value, context = "report") {
  if (typeof value === "number") {
    assert(Number.isFinite(value), `${context} contains a non-finite number`);
  } else if (Array.isArray(value)) {
    value.forEach((item, index) => finiteNumbersOnly(item, `${context}[${index}]`));
  } else if (value && typeof value === "object") {
    for (const [key, item] of Object.entries(value)) {
      finiteNumbersOnly(item, `${context}.${key}`);
    }
  }
}

function markdownMetric(markdown, label) {
  const escaped = label.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
  const match = markdown.match(new RegExp(`^\\| ${escaped} \\| ([0-9.]+)%? \\|$`, "mu"));
  assert(match, `missing promoted Markdown metric: ${label}`);
  return Number(match[1]);
}

const reportPath = safeRepositoryPath(relativeReportPath, "report path");
const reportText = await readFile(reportPath, "utf8");
const report = JSON.parse(reportText);
finiteNumbersOnly(report);

assert(report.schema_version === 1, "unsupported report schema_version");
assert(report.report_metadata?.kind === "historical-market-data-engineering-case-study", "unexpected report kind");
assert(Array.isArray(report.dataset?.schemas) && report.dataset.schemas.length === 4, "expected four dataset schemas");
assert(
  report.dataset.schemas.every(
    (schema) =>
      typeof schema.name === "string" &&
      typeof schema.label === "string" &&
      typeof schema.used_for === "string" &&
      schema.used_for.length > 0
  ),
  "every dataset schema must have string name, label, and used_for fields"
);
assert(Array.isArray(report.claims) && report.claims.length > 0, "claims must be non-empty");
assert(Array.isArray(report.benchmark?.results), "benchmark results must be an array");
assert(Array.isArray(report.benchmark?.unsupported_configurations), "unsupported configurations must be an array");
assert(Array.isArray(report.limitations) && report.limitations.length > 0, "limitations must be non-empty");
assert(report.synthetic?.book_replay?.evidence_status === "synthetic", "book replay must remain synthetic");
assert(report.synthetic?.sweep_scenarios?.evidence_status === "synthetic", "sweep scenarios must remain synthetic");

assert(
  report.dataset.schemas.reduce((total, schema) => total + schema.records, 0) === report.dataset.total_records,
  "dataset record total does not reconcile"
);
assert(
  report.dataset.schemas.reduce((total, schema) => total + schema.compressed_bytes, 0) ===
    report.dataset.compressed_bytes,
  "dataset compressed-byte total does not reconcile"
);
assert(
  report.dataset.schemas.reduce((total, schema) => total + schema.decoded_bytes, 0) ===
    report.dataset.decoded_bytes,
  "dataset decoded-byte total does not reconcile"
);

assert(
  report.validation.mbp10_updates_before_valid_baseline + report.validation.aligned_updates ===
    report.validation.mbp10_records_scanned,
  "validation denominator does not reconcile"
);
assert(report.validation.unmatched_mbp10_updates === 0, "eligible MBP-10 misses cannot be hidden");
assert(report.validation.exact_matches === report.validation.aligned_updates, "validation exact-match drift");
assert(report.validation.end_to_end_exact_match_pct === 100, "validation percentage drift");
assert(
  report.validation.discrepancy_classes.reduce((total, row) => total + row.updates, 0) ===
    report.validation.aligned_updates,
  "validation histogram does not reconcile"
);

assert(
  report.sweep.above_high_count + report.sweep.below_low_count === report.sweep.event_count,
  "sweep direction totals do not reconcile"
);
assert(report.sweep.parameters.tick_size_points === 0.25, "sweep tick-size conversion drift");
assert(report.parity.mbo_records === report.sweep.mbo_records_scanned, "parity MBO count drift");
assert(report.parity.event_count === report.sweep.event_count, "parity event count drift");
assert(report.parity.record_count_match === true, "record parity must be explicit");
assert(report.parity.event_field_match === true, "event-field parity must be explicit");

assert(report.benchmark.results.length === 31, "expected 31 measured benchmark configurations");
assert(report.benchmark.unsupported_configurations.length === 1, "expected one unsupported configuration");
assert(
  report.benchmark.measured_configuration_count === report.benchmark.results.length,
  "benchmark measured count drift"
);
assert(
  report.benchmark.unsupported_configuration_count === report.benchmark.unsupported_configurations.length,
  "benchmark unsupported count drift"
);

const claimIds = new Set();
const sourceCache = new Map();
const promotedClaimSources = new Map([
  ["bench/results.json", {
    results: report.benchmark.results,
    unsupported_configurations: report.benchmark.unsupported_configurations
  }],
  ["evidence/public/acquisition-summary.json", {
    dataset: report.dataset,
    schemas: report.dataset.schemas
  }],
  ["evidence/public/book-validation-summary.json", report.validation],
  ["evidence/public/parity-summary.json", report.parity],
  ["evidence/public/sweep-summary.json", report.sweep]
]);
for (const publicClaim of report.claims) {
  assert(typeof publicClaim.id === "string" && publicClaim.id.length > 0, "claim id is required");
  assert(!claimIds.has(publicClaim.id), `duplicate claim id: ${publicClaim.id}`);
  claimIds.add(publicClaim.id);
  assert(allowedStatuses.has(publicClaim.evidence_status), `unknown evidence status: ${publicClaim.evidence_status}`);
  assert(allowedUnits.has(publicClaim.unit), `unknown claim unit: ${publicClaim.unit}`);
  assert(typeof publicClaim.label === "string" && publicClaim.label.length > 0, `${publicClaim.id}: label is required`);
  assert(typeof publicClaim.display === "string" && publicClaim.display.length > 0, `${publicClaim.id}: display is required`);
  assert(
    typeof publicClaim.method_note === "string" && publicClaim.method_note.length > 0,
    `${publicClaim.id}: method_note is required`
  );
  assert(
    typeof publicClaim.limitations === "string" && publicClaim.limitations.length > 0,
    `${publicClaim.id}: limitations must be a non-empty string`
  );
  assert(publicClaim.evidence_status !== "synthetic", `${publicClaim.id}: measured claim list cannot contain synthetic data`);

  const absoluteSource = safeRepositoryPath(publicClaim.source_path, `${publicClaim.id}: source_path`);
  let source = sourceCache.get(publicClaim.source_path);
  if (!source) {
    if (existsSync(absoluteSource)) {
      await access(absoluteSource);
      source = JSON.parse(await readFile(absoluteSource, "utf8"));
    } else {
      source = promotedClaimSources.get(publicClaim.source_path);
      assert(source, `${publicClaim.id}: required claim source is absent: ${publicClaim.source_path}`);
    }
    sourceCache.set(publicClaim.source_path, source);
  }
  const sourceValue = resolveJsonPointer(source, publicClaim.source_locator);
  const expectedValue = calculatedSourceValue(sourceValue, publicClaim.calculation);
  assert(
    Object.is(publicClaim.value, expectedValue),
    `${publicClaim.id}: value drifted from ${publicClaim.source_path}${publicClaim.source_locator}`
  );
}

const inputPaths = new Set();
for (const input of report.generation?.inputs ?? []) {
  assert(typeof input.path === "string", "generation input path is required");
  assert(!inputPaths.has(input.path), `duplicate generation input: ${input.path}`);
  inputPaths.add(input.path);
  assert(/^[a-f0-9]{64}$/u.test(input.sha256), `${input.path}: invalid SHA-256`);
  const absoluteInput = safeRepositoryPath(input.path, "generation input");
  if (!existsSync(absoluteInput)) {
    assert(
      intentionallyPrivateGenerationInputs.has(input.path),
      `${input.path}: required generation input is absent`
    );
    continue;
  }
  const contents = await readFile(absoluteInput);
  const actualHash = createHash("sha256").update(contents).digest("hex");
  assert(actualHash === input.sha256, `${input.path}: generation input hash drift`);
}
assert(inputPaths.size > 0, "generation inputs must be recorded");

const validationMarkdown = await readFile(path.join(repoRoot, "docs/book-validation.md"), "utf8");
const validationDriftChecks = [
  ["MBO records scanned", report.validation.mbo_records_scanned],
  ["MBP-10 records scanned", report.validation.mbp10_records_scanned],
  ["MBO records withheld before baseline/recovery", report.validation.mbo_records_withheld_before_baseline],
  ["MBP-10 updates before valid MBO baseline", report.validation.mbp10_updates_before_valid_baseline],
  ["Aligned updates", report.validation.aligned_updates],
  ["Unmatched eligible MBO observations", report.validation.unmatched_mbo_observations],
  ["Unmatched MBP-10 updates", report.validation.unmatched_mbp10_updates],
  ["Exact price-and-size matches", report.validation.exact_matches],
  ["End-to-end exact match", report.validation.end_to_end_exact_match_pct]
];
for (const [label, expected] of validationDriftChecks) {
  assert(markdownMetric(validationMarkdown, label) === expected, `promoted validation drift: ${label}`);
}

const sweepMarkdown = await readFile(path.join(repoRoot, "docs/sweep-detection.md"), "utf8");
const sweepMatch = sweepMarkdown.match(
  /Scanned ([0-9]+) MBO records\.\s*\n- Emitted ([0-9]+) monotonic events: ([0-9]+) above prior highs and ([0-9]+) below prior lows\./u
);
assert(sweepMatch, "promoted sweep summary is missing");
assert(Number(sweepMatch[1]) === report.sweep.mbo_records_scanned, "promoted sweep MBO count drift");
assert(Number(sweepMatch[2]) === report.sweep.event_count, "promoted sweep event-count drift");
assert(Number(sweepMatch[3]) === report.sweep.above_high_count, "promoted above-high count drift");
assert(Number(sweepMatch[4]) === report.sweep.below_low_count, "promoted below-low count drift");

process.stdout.write(
  `public report data validation passed: ${relativeReportPath} (${report.claims.length} claims, ${report.benchmark.results.length} measured benchmark rows, ${report.benchmark.unsupported_configurations.length} unsupported)\n`
);
