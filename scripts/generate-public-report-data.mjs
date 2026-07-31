#!/usr/bin/env node

import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(scriptPath), "..");
const defaultOutput = "web/public/data/report.v1.json";

const sourcePaths = [
  "bench/machine.json",
  "bench/results.json",
  "config/live-session.json",
  "config/sweep.json",
  "evidence/public/acquisition-summary.json",
  "evidence/public/book-validation-summary.json",
  "evidence/public/parity-summary.json",
  "evidence/public/sweep-summary.json",
  "scripts/generate-public-report-data.mjs"
].sort();

function parseArguments(argv) {
  const options = {
    check: false,
    stdout: false,
    output: defaultOutput
  };

  for (const argument of argv) {
    if (argument === "--check") {
      options.check = true;
    } else if (argument === "--stdout") {
      options.stdout = true;
    } else if (argument.startsWith("--output=")) {
      options.output = argument.slice("--output=".length);
    } else {
      throw new Error(`unknown argument: ${argument}`);
    }
  }

  if (path.isAbsolute(options.output) || options.output.split(/[\\/]/u).includes("..")) {
    throw new Error("--output must be a repository-relative path without parent traversal");
  }

  return options;
}

async function readJson(relativePath) {
  const contents = await readFile(path.join(repoRoot, relativePath), "utf8");
  return JSON.parse(contents);
}

function sum(items, field) {
  return items.reduce((total, item) => total + item[field], 0);
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function selectBenchmark(results, schema) {
  const matches = results.filter(
    (row) =>
      row.schema === schema &&
      row.compression === "zstd" &&
      row.access === "streaming" &&
      row.concurrency === "single_thread"
  );
  assert(matches.length === 1, `expected one headline benchmark row for ${schema}`);
  return matches[0];
}

function validateInputs({ acquisition, benchmark, liveSession, sweepConfig, validation, sweep, parity }) {
  assert(acquisition.schema_version === "1.0.0", "unsupported acquisition evidence version");
  assert(validation.schema_version === "1.0.0", "unsupported validation evidence version");
  assert(sweep.schema_version === "1.0.0", "unsupported sweep evidence version");
  assert(parity.schema_version === "1.0.0", "unsupported parity evidence version");

  assert(acquisition.dataset.name === liveSession.dataset, "dataset name drift");
  assert(acquisition.dataset.symbol === liveSession.symbol, "dataset symbol drift");
  assert(acquisition.dataset.start === liveSession.start, "dataset start drift");
  assert(acquisition.dataset.end === liveSession.end, "dataset end drift");
  assert(liveSession.spend_cap_usd === 10, "unexpected spend cap");
  assert(acquisition.dataset.total_cost_usd <= liveSession.spend_cap_usd, "promoted cost exceeds spend cap");

  assert(sum(acquisition.schemas, "records") === acquisition.dataset.total_records, "record total drift");
  assert(
    sum(acquisition.schemas, "compressed_bytes") === acquisition.dataset.compressed_bytes,
    "compressed-byte total drift"
  );
  assert(
    sum(acquisition.schemas, "decoded_bytes") === acquisition.dataset.decoded_bytes,
    "decoded-byte total drift"
  );
  assert(
    Math.abs(sum(acquisition.schemas, "cost_usd") - acquisition.dataset.total_cost_usd) < 0.000001,
    "rounded per-schema costs do not reconcile with the conservative total"
  );

  for (const schema of acquisition.schemas) {
    const benchmarkRow = selectBenchmark(benchmark.results, schema.name);
    assert(benchmarkRow.messages_per_run === schema.records, `${schema.name} record count drift`);
    assert(benchmarkRow.wire_bytes_per_run === schema.compressed_bytes, `${schema.name} compressed-byte drift`);
    assert(benchmarkRow.decoded_bytes_per_run === schema.decoded_bytes, `${schema.name} decoded-byte drift`);
  }

  assert(
    validation.mbp10_updates_before_valid_baseline + validation.aligned_updates ===
      validation.mbp10_records_scanned,
    "validation denominator does not reconcile"
  );
  assert(validation.unmatched_mbp10_updates === 0, "public validation evidence has eligible MBP-10 misses");
  assert(validation.exact_matches === validation.aligned_updates, "exact-match count drift");
  assert(validation.end_to_end_exact_match_pct === 100, "end-to-end validation percentage drift");
  assert(
    validation.discrepancy_classes.reduce((total, row) => total + row.updates, 0) ===
      validation.aligned_updates,
    "validation discrepancy histogram does not reconcile"
  );

  assert(sweep.parameters.lookback_trades === sweepConfig.lookback_trades, "sweep lookback drift");
  assert(sweep.parameters.threshold_ticks === sweepConfig.threshold_ticks, "sweep threshold drift");
  assert(
    sweep.parameters.reversion_window_ms === sweepConfig.reversion_window_ms,
    "sweep reversion-window drift"
  );
  assert(sweepConfig.tick_size === 250000000, "unexpected fixed-price tick size");
  assert(sweep.parameters.tick_size_points === 0.25, "public tick-size conversion drift");
  assert(sweep.above_high_count + sweep.below_low_count === sweep.event_count, "sweep totals do not reconcile");
  assert(sweep.mbo_records_scanned === validation.mbo_records_scanned, "sweep MBO denominator drift");

  assert(parity.mbo_records === validation.mbo_records_scanned, "parity MBO count drift");
  assert(parity.event_count === sweep.event_count, "parity event count drift");
  assert(parity.record_count_match === true, "record parity is not proven");
  assert(parity.event_field_match === true, "event-field parity is not proven");

  assert(benchmark.results.length === 31, "expected 31 measured benchmark configurations");
  assert(
    benchmark.unsupported_configurations.length === 1,
    "expected one explicitly unsupported benchmark configuration"
  );
}

function claim({
  id,
  label,
  value,
  display,
  unit,
  evidenceStatus,
  sourcePath,
  sourceLocator,
  methodNote,
  limitations,
  calculation
}) {
  return {
    id,
    label,
    value,
    display,
    unit,
    evidence_status: evidenceStatus,
    source_path: sourcePath,
    source_locator: sourceLocator,
    method_note: methodNote,
    limitations,
    ...(calculation ? { calculation } : {})
  };
}

function syntheticBookReplay() {
  return {
    evidence_status: "synthetic",
    label: "Synthetic teaching example",
    description:
      "An invented ten-step sequence demonstrates baseline withholding, event-boundary comparison, invalidation, and recovery. It is not sampled from purchased data.",
    price_unit: "illustrative points",
    events: [
      {
        id: "book-01",
        at_ms: 0,
        action: "clear",
        side: null,
        price_points: null,
        size: null,
        comparison_view: "none",
        trust_state: "withheld",
        explanation: "A clear begins a new baseline. The book is not yet trustworthy.",
        book_after: { bids: [], asks: [] }
      },
      {
        id: "book-02",
        at_ms: 100,
        action: "snapshot_add",
        side: "bid",
        price_points: 100,
        size: 12,
        comparison_view: "none",
        trust_state: "withheld",
        explanation: "A bid arrives, but the snapshot event is incomplete.",
        book_after: { bids: [[100, 12]], asks: [] }
      },
      {
        id: "book-03",
        at_ms: 200,
        action: "snapshot_add_last",
        side: "ask",
        price_points: 100.25,
        size: 10,
        comparison_view: "final-event",
        trust_state: "valid",
        explanation: "The final snapshot record establishes a complete two-sided baseline.",
        book_after: { bids: [[100, 12]], asks: [[100.25, 10]] }
      },
      {
        id: "book-04",
        at_ms: 300,
        action: "add",
        side: "bid",
        price_points: 99.75,
        size: 6,
        comparison_view: "final-event",
        trust_state: "valid",
        explanation: "A lower bid is added without changing the top bid.",
        book_after: { bids: [[100, 12], [99.75, 6]], asks: [[100.25, 10]] }
      },
      {
        id: "book-05",
        at_ms: 400,
        action: "modify",
        side: "bid",
        price_points: 100,
        size: 9,
        comparison_view: "final-event",
        trust_state: "valid",
        explanation: "A state-changing event is compared after its complete event boundary.",
        book_after: { bids: [[100, 9], [99.75, 6]], asks: [[100.25, 10]] }
      },
      {
        id: "book-06",
        at_ms: 500,
        action: "trade",
        side: "ask",
        price_points: 100.25,
        size: 3,
        comparison_view: "pre-event",
        trust_state: "valid",
        explanation: "A trade is compared with the pre-event book before associated mutations.",
        book_after: { bids: [[100, 9], [99.75, 6]], asks: [[100.25, 10]] }
      },
      {
        id: "book-07",
        at_ms: 600,
        action: "cancel",
        side: "ask",
        price_points: 100.25,
        size: 3,
        comparison_view: "final-event",
        trust_state: "valid",
        explanation: "The paired mutation reduces displayed ask size after the trade comparison.",
        book_after: { bids: [[100, 9], [99.75, 6]], asks: [[100.25, 7]] }
      },
      {
        id: "book-08",
        at_ms: 700,
        action: "invalidate",
        side: null,
        price_points: null,
        size: null,
        comparison_view: "none",
        trust_state: "invalid",
        explanation: "A malformed or incomplete boundary invalidates the reconstructed state.",
        book_after: { bids: [], asks: [] }
      },
      {
        id: "book-09",
        at_ms: 800,
        action: "snapshot_add",
        side: "bid",
        price_points: 100.5,
        size: 8,
        comparison_view: "none",
        trust_state: "withheld",
        explanation: "Recovery begins from a new complete snapshot, not from stale state.",
        book_after: { bids: [[100.5, 8]], asks: [] }
      },
      {
        id: "book-10",
        at_ms: 900,
        action: "snapshot_add_last",
        side: "ask",
        price_points: 100.75,
        size: 11,
        comparison_view: "final-event",
        trust_state: "valid",
        explanation: "The completed replacement snapshot restores trustworthy state.",
        book_after: { bids: [[100.5, 8]], asks: [[100.75, 11]] }
      }
    ]
  };
}

function syntheticSweepScenarios() {
  return {
    evidence_status: "synthetic",
    label: "Synthetic heuristic lab",
    description:
      "Invented paths explain candidate creation, reversion, expiry, and direction. Controls may change these examples but never the measured result.",
    scenarios: [
      {
        id: "above-high-reverted",
        label: "Above prior high, reverted",
        outcome: "emitted",
        direction: "above_high",
        points: [
          { at_ms: 0, price_points: 100 },
          { at_ms: 500, price_points: 101 },
          { at_ms: 2100, price_points: 100 }
        ]
      },
      {
        id: "below-low-reverted",
        label: "Below prior low, reverted",
        outcome: "emitted",
        direction: "below_low",
        points: [
          { at_ms: 0, price_points: 100 },
          { at_ms: 700, price_points: 99 },
          { at_ms: 2600, price_points: 100 }
        ]
      },
      {
        id: "above-high-expired",
        label: "Above prior high, expired",
        outcome: "expired",
        direction: "above_high",
        points: [
          { at_ms: 0, price_points: 100 },
          { at_ms: 600, price_points: 101 },
          { at_ms: 5700, price_points: 100.25 }
        ]
      },
      {
        id: "threshold-not-reached",
        label: "Threshold not reached",
        outcome: "no_candidate",
        direction: null,
        points: [
          { at_ms: 0, price_points: 100 },
          { at_ms: 800, price_points: 100.75 },
          { at_ms: 1600, price_points: 100 }
        ]
      }
    ]
  };
}

async function buildReport() {
  const [
    machine,
    benchmark,
    liveSession,
    sweepConfig,
    acquisition,
    validation,
    parity,
    sweep
  ] = await Promise.all([
    readJson("bench/machine.json"),
    readJson("bench/results.json"),
    readJson("config/live-session.json"),
    readJson("config/sweep.json"),
    readJson("evidence/public/acquisition-summary.json"),
    readJson("evidence/public/book-validation-summary.json"),
    readJson("evidence/public/parity-summary.json"),
    readJson("evidence/public/sweep-summary.json")
  ]);

  validateInputs({ acquisition, benchmark, liveSession, sweepConfig, validation, sweep, parity });

  const headlineBenchmarkIndex = benchmark.results.findIndex(
    (row) =>
      row.schema === "mbo" &&
      row.compression === "zstd" &&
      row.access === "streaming" &&
      row.concurrency === "single_thread"
  );
  assert(headlineBenchmarkIndex >= 0, "headline MBO benchmark is missing");
  const headlineBenchmark = benchmark.results[headlineBenchmarkIndex];

  const generationInputs = await Promise.all(
    sourcePaths.map(async (relativePath) => {
      const bytes = await readFile(path.join(repoRoot, relativePath));
      return {
        path: relativePath,
        sha256: createHash("sha256").update(bytes).digest("hex")
      };
    })
  );

  return {
    schema_version: 1,
    report_metadata: {
      title: "Reconstructing ES from 58,988,994 market events",
      subtitle:
        "A Rust and Node market-data engineering case study: bounded acquisition, streaming decode, order-book reconstruction, independent validation, and measured performance.",
      kind: "historical-market-data-engineering-case-study",
      evidence_scope: "One bounded 23-hour ES Globex session and one measured host"
    },
    dataset: {
      ...acquisition.dataset,
      schemas: acquisition.schemas
    },
    claims: [
      claim({
        id: "dataset.total_records",
        label: "Records streamed",
        value: acquisition.dataset.total_records,
        display: "58,988,994",
        unit: "records",
        evidenceStatus: "derived-live",
        sourcePath: "evidence/public/acquisition-summary.json",
        sourceLocator: "/dataset/total_records",
        methodNote: "Sum of the four schema record counts after checksum, schema, and count verification.",
        limitations: "Bounded to the named historical session and four acquired schemas."
      }),
      claim({
        id: "dataset.total_cost_usd",
        label: "Conservatively counted acquisition cost",
        value: acquisition.dataset.total_cost_usd,
        display: "$9.011640",
        unit: "USD",
        evidenceStatus: "measured-live",
        sourcePath: "evidence/public/acquisition-summary.json",
        sourceLocator: "/dataset/total_cost_usd",
        methodNote: acquisition.cost_accounting.method,
        limitations:
          "Per-schema public display costs are rounded; this total preserves the promoted conservative ledger value."
      }),
      claim({
        id: "validation.exact_matches",
        label: "Exact eligible MBP-10 updates",
        value: validation.exact_matches,
        display: "23,961,616 / 23,961,616",
        unit: "records",
        evidenceStatus: "measured-live",
        sourcePath: "evidence/public/book-validation-summary.json",
        sourceLocator: "/exact_matches",
        methodNote: validation.methodology,
        limitations: "Top-of-book agreement only; this is not full-depth, routing, or execution-quality proof."
      }),
      claim({
        id: "validation.end_to_end_exact_match_pct",
        label: "End-to-end exact match",
        value: validation.end_to_end_exact_match_pct,
        display: "100.000000%",
        unit: "percent",
        evidenceStatus: "derived-live",
        sourcePath: "evidence/public/book-validation-summary.json",
        sourceLocator: "/end_to_end_exact_match_pct",
        methodNote: "Exact price-and-size matches divided by all eligible MBP-10 updates.",
        limitations: "Measured after the explicit complete-baseline boundary for one session."
      }),
      claim({
        id: "benchmark.mbo_streaming_messages_per_second",
        label: "Compressed MBO streaming throughput",
        value: headlineBenchmark.messages_per_second_median,
        display: "3,454,156",
        unit: "messages/s",
        evidenceStatus: "measured-live",
        sourcePath: "bench/results.json",
        sourceLocator: `/results/${headlineBenchmarkIndex}/messages_per_second_median`,
        methodNote:
          "Median of five measured fresh child processes after one discarded warmup; compressed, streaming, single-thread MBO.",
        limitations: "Warm page cache and one measured host; not a cross-platform guarantee."
      }),
      claim({
        id: "benchmark.mbo_streaming_decoded_mib_per_second",
        label: "Decoded MBO throughput",
        value: headlineBenchmark.decoded_mib_per_second_median,
        display: "184.47",
        unit: "MiB/s",
        evidenceStatus: "measured-live",
        sourcePath: "bench/results.json",
        sourceLocator: `/results/${headlineBenchmarkIndex}/decoded_mib_per_second_median`,
        methodNote:
          "Decoded throughput for the same compressed, streaming, single-thread MBO benchmark row.",
        limitations: "Warm page cache and one measured host; not a cross-platform guarantee."
      }),
      claim({
        id: "sweep.event_count",
        label: "Heuristic sweep events",
        value: sweep.event_count,
        display: "11",
        unit: "events",
        evidenceStatus: "measured-live",
        sourcePath: "evidence/public/sweep-summary.json",
        sourceLocator: "/event_count",
        methodNote: sweep.methodology,
        limitations: sweep.limitations.join(" ")
      }),
      claim({
        id: "benchmark.measured_configuration_count",
        label: "Measured benchmark configurations",
        value: benchmark.results.length,
        display: "31",
        unit: "configurations",
        evidenceStatus: "derived-live",
        sourcePath: "bench/results.json",
        sourceLocator: "/results",
        methodNote: "Count of measured configuration rows in the promoted benchmark matrix.",
        limitations: "One additional high-memory configuration is explicitly unsupported.",
        calculation: { kind: "array_length" }
      }),
      claim({
        id: "parity.mbo_records",
        label: "Rust and Node parity records",
        value: parity.mbo_records,
        display: "33,438,049",
        unit: "records",
        evidenceStatus: "measured-live",
        sourcePath: "evidence/public/parity-summary.json",
        sourceLocator: "/mbo_records",
        methodNote: parity.methodology,
        limitations: parity.limitations.join(" ")
      })
    ],
    validation: {
      mbo_records_scanned: validation.mbo_records_scanned,
      mbp10_records_scanned: validation.mbp10_records_scanned,
      mbo_records_withheld_before_baseline: validation.mbo_records_withheld_before_baseline,
      mbp10_updates_before_valid_baseline: validation.mbp10_updates_before_valid_baseline,
      aligned_updates: validation.aligned_updates,
      unmatched_mbo_observations: validation.unmatched_mbo_observations,
      unmatched_mbp10_updates: validation.unmatched_mbp10_updates,
      exact_matches: validation.exact_matches,
      end_to_end_exact_match_pct: validation.end_to_end_exact_match_pct,
      methodology: validation.methodology,
      discrepancy_classes: validation.discrepancy_classes
    },
    sweep: {
      parameters: sweep.parameters,
      event_count: sweep.event_count,
      above_high_count: sweep.above_high_count,
      below_low_count: sweep.below_low_count,
      mbo_records_scanned: sweep.mbo_records_scanned,
      methodology: sweep.methodology,
      limitations: sweep.limitations
    },
    parity: {
      mbo_records: parity.mbo_records,
      event_count: parity.event_count,
      record_count_match: parity.record_count_match,
      event_field_match: parity.event_field_match,
      methodology: parity.methodology,
      limitations: parity.limitations
    },
    benchmark: {
      generated_at: benchmark.generated_at,
      methodology: benchmark.methodology,
      capabilities: benchmark.capabilities,
      results: benchmark.results,
      unsupported_configurations: benchmark.unsupported_configurations,
      measured_configuration_count: benchmark.results.length,
      unsupported_configuration_count: benchmark.unsupported_configurations.length,
      machine: {
        cpu_model: machine.cpu.model,
        physical_cores: machine.cpu.physical_cores,
        logical_cores: machine.cpu.logical_cores,
        installed_memory_bytes: machine.memory.installed_bytes,
        os_name: machine.os.name,
        os_architecture: machine.os.architecture,
        verification_runtime: machine.verification_runtime.name,
        verification_target: machine.verification_runtime.target,
        idle_status: machine.idle_status
      }
    },
    limitations: [
      "The measured evidence covers one historical ES instrument session and one host.",
      "Benchmark measurements used a warm page cache; cold-cache performance is unsupported.",
      "Validation proves reconstructed top-of-book equality, not full-depth equality.",
      "The sweep detector is a transparent heuristic, not a signal, P&L claim, or execution recommendation.",
      "The report does not claim hidden-liquidity estimation, exchange connectivity, routing, or production trading.",
      "Provider timeout and interrupted-download behavior has not been fault-injected against the paid API.",
      "Hosted cross-platform native-package and public deployment proof is not yet part of this evidence."
    ],
    synthetic: {
      book_replay: syntheticBookReplay(),
      sweep_scenarios: syntheticSweepScenarios()
    },
    generation: {
      algorithm: "sha256",
      deterministic: true,
      inputs: generationInputs
    }
  };
}

const options = parseArguments(process.argv.slice(2));
const report = await buildReport();
const serialized = `${JSON.stringify(report, null, 2)}\n`;
const outputPath = path.join(repoRoot, options.output);

if (options.check) {
  let existing;
  try {
    existing = await readFile(outputPath, "utf8");
  } catch (error) {
    if (error && error.code === "ENOENT") {
      throw new Error(`${options.output} does not exist; run the generator first`);
    }
    throw error;
  }
  assert(existing === serialized, `${options.output} is stale; regenerate it`);
}

if (!options.check && !options.stdout) {
  await mkdir(path.dirname(outputPath), { recursive: true });
  await writeFile(outputPath, serialized, "utf8");
}

if (options.stdout) {
  process.stdout.write(serialized);
} else {
  const digest = createHash("sha256").update(serialized).digest("hex");
  process.stdout.write(
    `${options.check ? "public report data is current" : "generated public report data"}: ${options.output} (${digest})\n`
  );
}
