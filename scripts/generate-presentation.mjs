#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const readText = (path) => readFileSync(join(root, path), "utf8");
const readJson = (path) => JSON.parse(readText(path));
const write = (path, value) => writeFileSync(join(root, path), value.replace(/\r\n/gu, "\n"), "utf8");
const number = new Intl.NumberFormat("en-US", { maximumFractionDigits: 0 });
const decimal = new Intl.NumberFormat("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });

const benchmark = readJson("bench/results.json");
const machine = readJson("bench/machine.json");
const validation = readText("docs/book-validation.md");
const sweep = readText("docs/sweep-detection.md");
const sampleDecode = readJson("data/sample/decode-stats.json");
const sampleValidation = readJson("data/sample/book-validation.json");
const sampleSweeps = readJson("data/sample/sweep-summary.json");

if (benchmark.source !== "live" || sampleValidation.source !== "synthetic" || sampleSweeps.source !== "synthetic") {
  throw new Error("presentation generation requires live benchmark evidence and synthetic demo evidence with explicit provenance");
}

const requireMatch = (text, pattern, label) => {
  const match = text.match(pattern);
  if (!match) throw new Error(`unable to extract ${label}`);
  return match[1];
};

const live = {
  exactPct: requireMatch(validation, /\*\*Result:\*\* ([\d.]+)% end-to-end exact/u, "validation rate"),
  aligned: Number(requireMatch(validation, /\| Aligned updates \| (\d+) \|/u, "aligned updates")),
  exact: Number(requireMatch(validation, /\| Exact price-and-size matches \| (\d+) \|/u, "exact matches")),
  mbo: Number(requireMatch(validation, /\| MBO records scanned \| (\d+) \|/u, "MBO records")),
  mbp10: Number(requireMatch(validation, /\| MBP-10 records scanned \| (\d+) \|/u, "MBP-10 records")),
  sweeps: Number(requireMatch(sweep, /Emitted (\d+) monotonic events/u, "sweep count")),
  above: Number(requireMatch(sweep, /events: (\d+) above prior highs/u, "above-high sweep count")),
  below: Number(requireMatch(sweep, /and (\d+) below prior lows/u, "below-low sweep count")),
};

const schemas = ["mbo", "mbp-10", "trades", "ohlcv-1m"];
const comparable = schemas.map((schema) => {
  const result = benchmark.results.find(
    (row) =>
      row.schema === schema &&
      row.compression === "zstd" &&
      row.access === "streaming" &&
      row.concurrency === "single_thread",
  );
  if (!result) throw new Error(`missing comparable benchmark row for ${schema}`);
  return result;
});

const histogramSection = validation.split("## Discrepancy histogram")[1]?.split("## Failure-mode analysis")[0] ?? "";
const histogram = [...histogramSection.matchAll(/\| `([^`]+)` \| (\d+) \|/gu)].map((match) => ({
  label: match[1],
  count: Number(match[2]),
}));
if (histogram.length === 0) throw new Error("missing discrepancy histogram");

const throughputTable = comparable
  .map(
    (row) =>
      `| ${row.schema} | ${number.format(row.messages_per_run)} | ${number.format(row.messages_per_second_median)} | ${decimal.format(row.decoded_mib_per_second_median)} | ${decimal.format(row.peak_rss_mib_median)} |`,
  )
  .join("\n");

const summary = `# Technical summary

## What was built

This repository implements a bounded, streaming Rust pipeline for Databento Binary Encoding data: checksum-verified acquisition, schema-aware decoding, market-by-order reconstruction, exact top-of-book comparison against exchange MBP-10, a parameterized sweep/reversion heuristic, a process-isolated benchmark harness, and a Node-API binding. The core owns decoding and state transitions; the CLI owns files, acquisition gates, reports, and orchestration; the binding exposes streaming iterators without duplicating market logic.

## What was measured

The live evidence is one \`GLBX.MDP3\` \`ES.v.0\` session from 2025-04-03 22:00 UTC through 2025-04-04 21:00 UTC. Reconstruction scanned ${number.format(live.mbo)} MBO and ${number.format(live.mbp10)} MBP-10 records. All ${number.format(live.exact)} of ${number.format(live.aligned)} eligible exchange updates matched reconstructed bid/ask price and size (${live.exactPct}% end to end). The table below uses the comparable live Zstd, streaming, single-thread configuration; each value is the median of five measured fresh child processes after one discarded warmup.

| Schema | Records/run | Messages/s | Decoded MiB/s | Peak RSS MiB |
| --- | ---: | ---: | ---: | ---: |
${throughputTable}

Host: ${machine.cpu.model}; ${machine.cpu.physical_cores} physical/${machine.cpu.logical_cores} logical cores; ${Math.round(machine.memory.installed_bytes / 2 ** 30)} GiB ${machine.memory.type}-${machine.memory.configured_speed_mt_s}; ${machine.workspace_disk.media_type}/${machine.workspace_disk.bus_type}; benchmark runtime ${machine.verification_runtime.name}.

## What was learned

The important correctness boundary was not applying every MBO record immediately. Reconstruction remains invalid until a complete clear/snapshot ending at the DBN event boundary; trade records compare the pre-event book, while state-changing events compare the final book. Unmatched eligible MBP-10 updates stay in the denominator. This produced ${live.exactPct}% exact agreement without treating pre-baseline or incomplete state as valid.

The performance result was similarly boundary-sensitive. A single compressed MBO stream sustained ${number.format(comparable[0].messages_per_second_median)} messages/s (${decimal.format(comparable[0].decoded_mib_per_second_median)} decoded MiB/s), while independent-stream parallelism is reported separately rather than implying one Zstd stream is splittable. One raw MBP-10 four-buffer case is explicitly unsupported because its planned ${decimal.format(benchmark.unsupported_configurations[0] ? Number(benchmark.unsupported_configurations[0].reason.match(/total (\d+) bytes/u)?.[1] ?? 0) / 2 ** 30 : 0)} GiB allocation exceeds the 16 GiB safety gate.

## Production boundaries

The live dataset covers one instrument session, warm page cache, one CPU/OS combination, and top-of-book equality—not exchange connectivity, order routing, P&L, hidden liquidity, or trading performance. The ${live.sweeps} detected events (${live.above} above prior highs, ${live.below} below prior lows) are transparent heuristic output, not signals. Sample/CI evidence is deterministic synthetic data and is labeled as such. Exact regeneration starts at \`scripts/verify.sh\`; the full restored-corpus gate is \`scripts/verify.sh --full\`.
`;

const post = `${number.format(live.exact)} of ${number.format(live.aligned)} eligible exchange top-of-book updates matched a Rust reconstruction exactly—bid and ask price and size—across a live ES session.

The implementation streamed ${number.format(live.mbo)} Databento MBO records, withheld state until a complete snapshot boundary, compared trade events against the pre-event book, and kept unmatched eligible MBP-10 updates in the denominator. End-to-end exact agreement was ${live.exactPct}%.

On a Ryzen 7 7700X under WSL 2, the comparable compressed single-thread MBO run reached a five-run median of ${number.format(comparable[0].messages_per_second_median)} messages/s (${decimal.format(comparable[0].decoded_mib_per_second_median)} decoded MiB/s). The benchmark includes process isolation, warmup discard, p95, variance, and peak RSS; unsafe high-memory configurations report unsupported rather than producing a number.

The repository also includes deterministic four-schema fixtures, malformed-input/property tests, an exact Rust 1.88 gate, Node-API parity, generated reports, and one-command verification. The sweep output is a documented heuristic, not a trading signal.`;

const words = post.trim().split(/\s+/u).length;
if (words >= 200) throw new Error(`post draft is ${words} words; expected fewer than 200`);

const sampleRecords = sampleDecode.files.reduce((total, file) => total + file.record_count, 0);
const sampleSchemaCounts = sampleDecode.files.map((file) => `${file.schema}=${file.record_count}`).join(", ");
const demo = `# Five-minute demo

This walkthrough uses the committed deterministic generator. Its provenance is \`synthetic\`; it does not download or impersonate live market data.

## 1. Build once

Prerequisites are Rust 1.88 or newer and Node.js 18 or newer. PowerShell is supported on Windows hosts that permit locally compiled Rust executables; Ubuntu on WSL 2 is the documented fallback when Windows Application Control blocks them.

\`\`\`sh
cargo build --release --locked -p dbn-es-bench --bin dbn-es-bench
\`\`\`

The first dependency compilation is machine- and cache-dependent. The sub-60-second gate applies to the bounded demo workload after this standard build.

## 2. Run the pipeline

\`\`\`sh
scripts/demo.sh
\`\`\`

The script creates a disposable workspace, generates all four schemas, streams them once, validates reconstructed top of book, runs the sweep heuristic, prints a compact result, and removes successful output. Set \`DBN_ES_KEEP_DEMO=1\` to retain evidence; failed runs are always retained.

Expected result (the final elapsed value varies and must remain below 60):

\`\`\`text
DBN ES bench demo
provenance: synthetic (not market data)
decode: ${sampleRecords} records (${sampleSchemaCounts}); timestamp regressions=0
validation: ${sampleValidation.exact_matches}/${sampleValidation.aligned_updates} exact; end-to-end=${sampleValidation.end_to_end_exact_match_pct.toFixed(6)}%
sweeps: ${sampleSweeps.event_count} total (${sampleSweeps.above_high_count} above high, ${sampleSweeps.below_low_count} below low)
  above_high: displacement=4 ticks; duration=100000000 ns; resting_size_consumed=1
workload_seconds: <60
\`\`\`

## 3. Inspect the production evidence

The demo proves the clean-checkout execution path. It is not the live benchmark. The promoted live evidence is in \`bench/results.md\`, \`docs/book-validation.md\`, and \`docs/sweep-detection.md\`; regenerate it only from a checksum-verified restored corpus with \`scripts/verify.sh --full\`.
`;

const escapeHtml = (value) => String(value).replace(/[&<>"']/gu, (character) => ({
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
})[character]);

const maxThroughput = Math.max(...comparable.map((row) => row.decoded_mib_per_second_median));
const throughputBars = comparable.map((row, index) => {
  const y = 22 + index * 66;
  const width = (row.decoded_mib_per_second_median / maxThroughput) * 620;
  return `<g><text x="0" y="${y + 19}" class="label">${escapeHtml(row.schema)}</text><rect x="95" y="${y}" width="${width.toFixed(2)}" height="28" rx="4"/><text x="${(105 + width).toFixed(2)}" y="${y + 19}" class="value">${decimal.format(row.decoded_mib_per_second_median)} MiB/s</text></g>`;
}).join("");

const maxHistogram = Math.max(...histogram.map((row) => row.count));
const histogramBars = histogram.map((row, index) => {
  const y = 22 + index * 66;
  const width = (row.count / maxHistogram) * 620;
  return `<g><text x="0" y="${y + 19}" class="label">${escapeHtml(row.label)}</text><rect x="95" y="${y}" width="${width.toFixed(2)}" height="28" rx="4"/><text x="${(105 + width).toFixed(2)}" y="${y + 19}" class="value">${number.format(row.count)}</text></g>`;
}).join("");

const htmlRows = comparable.map((row) => `<tr><td>${escapeHtml(row.schema)}</td><td>${number.format(row.messages_per_run)}</td><td>${number.format(row.messages_per_second_median)}</td><td>${decimal.format(row.decoded_mib_per_second_median)}</td><td>${decimal.format(row.peak_rss_mib_median)}</td></tr>`).join("");
const histogramRows = histogram.map((row) => `<tr><td>${escapeHtml(row.label)}</td><td>${number.format(row.count)}</td></tr>`).join("");

const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>DBN ES benchmark results</title>
<style>
:root{color-scheme:dark;--bg:#0b1118;--panel:#121c27;--ink:#f3f7fa;--muted:#a9b9c7;--line:#2b3b4b;--accent:#53d6a4;--accent2:#73a8ff}*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font:16px/1.55 ui-monospace,SFMono-Regular,Consolas,monospace}main{width:min(1080px,calc(100% - 32px));margin:0 auto;padding:64px 0 80px}h1{font:700 clamp(2.1rem,6vw,4.8rem)/.98 system-ui,sans-serif;letter-spacing:-.05em;margin:0 0 20px;max-width:900px}.lede{color:var(--muted);max-width:820px;font-size:1.05rem}.metrics{display:grid;grid-template-columns:repeat(auto-fit,minmax(210px,1fr));gap:12px;margin:36px 0}.metric,.panel{border:1px solid var(--line);background:var(--panel);border-radius:12px}.metric{padding:18px}.metric strong{display:block;font:700 1.65rem system-ui,sans-serif;color:var(--accent)}.metric span,.note{color:var(--muted);font-size:.88rem}.panel{padding:24px;margin:18px 0;overflow:auto}h2{font:650 1.35rem system-ui,sans-serif;margin:0 0 10px}svg{width:100%;min-width:760px;height:auto;display:block}rect{fill:var(--accent2)}.label,.value{fill:var(--ink);font:14px ui-monospace,monospace}.value{fill:var(--muted)}table{border-collapse:collapse;width:100%;margin-top:18px;font-size:.9rem}th,td{text-align:right;padding:9px 12px;border-bottom:1px solid var(--line);white-space:nowrap}th:first-child,td:first-child{text-align:left}th{color:var(--muted);font-weight:500}code{color:var(--accent)}footer{color:var(--muted);margin-top:32px;font-size:.82rem}@media(max-width:600px){main{padding-top:36px}.panel{padding:16px}}
</style>
</head>
<body><main>
<h1>Streaming DBN decode, measured against the exchange view.</h1>
<p class="lede">Live <code>GLBX.MDP3</code> <code>ES.v.0</code> evidence. Throughput is the median of five measured fresh child processes after one discarded warmup; validation keeps unmatched eligible exchange updates in the denominator.</p>
<section class="metrics" aria-label="Headline results">
<div class="metric"><strong>${live.exactPct}%</strong><span>end-to-end exact top-of-book match</span></div>
<div class="metric"><strong>${number.format(live.exact)}</strong><span>exact eligible MBP-10 updates</span></div>
<div class="metric"><strong>${number.format(comparable[0].messages_per_second_median)}</strong><span>MBO messages/s, Zstd streaming single thread</span></div>
<div class="metric"><strong>${live.sweeps}</strong><span>documented heuristic events, not signals</span></div>
</section>
<section class="panel"><h2>Decoded throughput by schema</h2><p class="note">Comparable Zstd streaming, single-thread rows. Bar length is decoded MiB/s.</p><svg viewBox="0 0 900 286" role="img" aria-label="Decoded throughput by schema">${throughputBars}</svg><table><thead><tr><th>Schema</th><th>Records/run</th><th>Messages/s</th><th>Decoded MiB/s</th><th>Peak RSS MiB</th></tr></thead><tbody>${htmlRows}</tbody></table></section>
<section class="panel"><h2>Top-of-book discrepancy histogram</h2><p class="note">Classifications across ${number.format(live.aligned)} eligible live exchange updates.</p><svg viewBox="0 0 900 ${Math.max(88, histogram.length * 66 + 22)}" role="img" aria-label="Book reconstruction discrepancy histogram">${histogramBars}</svg><table><thead><tr><th>Classification</th><th>Updates</th></tr></thead><tbody>${histogramRows}</tbody></table></section>
<section class="panel"><h2>Method boundary</h2><p>MBO state is withheld until a complete snapshot event. Trade records compare the pre-event book; mutations compare final event state. This page does not report cold-cache performance, order-routing behavior, P&amp;L, hidden liquidity, or trading-signal quality.</p><p class="note">Host: ${escapeHtml(machine.cpu.model)}, ${machine.cpu.physical_cores}C/${machine.cpu.logical_cores}T, ${Math.round(machine.memory.installed_bytes / 2 ** 30)} GiB ${escapeHtml(machine.memory.type)}-${machine.memory.configured_speed_mt_s}, ${escapeHtml(machine.verification_runtime.name)}.</p></section>
<footer>Self-contained offline artifact generated by <code>scripts/generate-presentation.mjs</code> from <code>bench/results.json</code>, <code>bench/machine.json</code>, <code>docs/book-validation.md</code>, and <code>docs/sweep-detection.md</code>. Benchmark captured ${escapeHtml(benchmark.generated_at)}. No external scripts, fonts, telemetry, or network requests.</footer>
</main></body></html>
`;

write("docs/summary.md", summary);
write("docs/post-draft.md", `# Public post draft\n\n${post}\n\n_Word count: ${words}._\n`);
write("docs/DEMO.md", demo);
write("docs/results.html", html);
