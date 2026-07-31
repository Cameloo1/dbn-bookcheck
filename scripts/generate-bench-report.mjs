#!/usr/bin/env node

import { readFile, writeFile } from "node:fs/promises";

const inputPath = process.argv[2] ?? "bench/results.json";
const outputPath = process.argv[3] ?? "bench/results.md";

const report = JSON.parse(await readFile(inputPath, "utf8"));
const machine = JSON.parse(await readFile(report.machine_file, "utf8"));

if (report.version !== 1 || !Array.isArray(report.results) || report.results.length === 0) {
  throw new Error("benchmark report must contain version 1 measured results");
}

const fmt = (value, digits = 2) => Number(value).toFixed(digits);
const label = (value) => value.replaceAll("_", " ");
const headline = report.results.find(
  (row) =>
    row.schema === "mbo" &&
    row.compression === "zstd" &&
    row.access === "streaming" &&
    row.concurrency === "single_thread",
);

if (!headline) {
  throw new Error("missing required single-threaded streaming MBO headline row");
}

const lines = [
  "# DBN ES benchmark results",
  "",
  `The headline measured path streamed the live compressed MBO session at **${fmt(headline.messages_per_second_median, 0)} messages/s median** (${fmt(headline.decoded_mib_per_second_median)} decoded MiB/s, ${fmt(headline.nanoseconds_per_message_median)} ns/message). This is an observed result on the machine below, not a cross-platform guarantee.`,
  "",
  "## Reproduce",
  "",
  "```sh",
  "cargo run --release -p dbn-es-bench --bin dbn-es-benchmark -- run",
  "node scripts/generate-bench-report.mjs",
  "```",
  "",
  `The raw benchmark JSON and machine capture are intentionally kept out of the public repository. A full local verification run regenerates this Markdown byte-identically from \`${inputPath.replaceAll("\\", "/")}\` and the private machine capture; the sanitized, versioned public evidence is published at \`web/public/data/report.v1.json\`.`,
  "",
  "## Environment",
  "",
  `- Generated: ${report.generated_at}`,
  `- Data: ${report.source}`,
  `- CPU: ${machine.cpu.model} (${machine.cpu.physical_cores} physical, ${machine.cpu.logical_cores} logical cores)`,
  `- Memory: ${fmt(machine.memory.installed_bytes / 1024 ** 3, 0)} GiB ${machine.memory.type}-${machine.memory.configured_speed_mt_s}`,
  `- Storage: ${machine.workspace_disk.model} ${machine.workspace_disk.bus_type} ${machine.workspace_disk.media_type}`,
  `- Runtime: ${machine.verification_runtime.name}, kernel ${machine.verification_runtime.kernel}, rustc ${machine.verification_runtime.rustc}`,
  `- Profile/cache: ${report.profile}, ${report.cache_condition} page cache; host idle status ${machine.idle_status}`,
  "",
  "## Method",
  "",
  report.methodology,
  "",
  "Rates in parallel rows are aggregate rates over independent identical streams. The same input is intentionally decoded once per worker; these rows do not claim one Zstd stream can be split.",
  "",
  "## Capability matrix",
  "",
  "| Capability | Status | Detail |",
  "| --- | --- | --- |",
  ...report.capabilities.map(
    (capability) =>
      `| ${label(capability.feature)} | ${label(capability.status)} | ${capability.detail} |`,
  ),
  "",
  "## Measured configurations",
  "",
  "| Schema | Compression | Access | Concurrency | Runs | Msg/s median | Msg/s p95 | Wire MiB/s | Decoded MiB/s | ns/msg | Peak RSS MiB median/max |",
  "| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
  ...report.results.map((row) => {
    const rss =
      row.peak_rss_mib_median === null
        ? "not instrumentable"
        : `${fmt(row.peak_rss_mib_median)}/${fmt(row.peak_rss_mib_max)}`;
    return `| ${row.schema} | ${row.compression} | ${label(row.access)} | ${label(row.concurrency)} (${row.threads}) | ${row.measured_runs} | ${fmt(row.messages_per_second_median, 0)} | ${fmt(row.messages_per_second_p95, 0)} | ${fmt(row.wire_mib_per_second_median)} | ${fmt(row.decoded_mib_per_second_median)} | ${fmt(row.nanoseconds_per_message_median)} | ${rss} |`;
  }),
  "",
  "Each row also records elapsed-time median/p95/population standard deviation, message-rate population standard deviation, byte counts, discarded warmups, and a null allocation count in the promoted public report.",
];

if (report.unsupported_configurations.length > 0) {
  lines.push(
    "",
    "## Unsupported configurations",
    "",
    "| Schema | Compression | Access | Concurrency | Reason |",
    "| --- | --- | --- | --- | --- |",
    ...report.unsupported_configurations.map(
      (row) =>
        `| ${row.schema} | ${row.compression} | ${label(row.access)} | ${label(row.concurrency)} | ${row.reason} |`,
    ),
  );
}

lines.push("");
await writeFile(outputPath, lines.join("\n"), "utf8");
