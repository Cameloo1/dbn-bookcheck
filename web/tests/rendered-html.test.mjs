import assert from "node:assert/strict";
import { readFile, readdir, stat } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, URL } from "node:url";

const expectedSections = [
  "overview",
  "data",
  "pipeline",
  "decode",
  "book",
  "validation",
  "sweeps",
  "benchmarks",
  "parity",
  "limits",
];

const webRoot = fileURLToPath(new URL("../", import.meta.url));
const distRoot = path.join(webRoot, "dist");
const reportPath = path.join(distRoot, "data", "report.v1.json");
const privatePathPattern =
  /(?:\b[A-Za-z]:[\\/]|\\\\[A-Za-z0-9._-]+\\[A-Za-z0-9$._-]+(?:\\[^\s"'<>|]+)*|\/(?:Users|home)\/[^/\s]+)/u;
const rawPayloadPattern = /\.(?:dbn|dbn\.zst|parquet|feather)(?:["'\s<]|$)/iu;
const credentialPattern =
  /(?:DATABENTO_API_KEY|OPENAI_API_KEY|(?:db|sk)-(?:live-)?[A-Za-z0-9_-]{20,})/u;

async function listFiles(root) {
  const files = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const absolute = path.join(root, entry.name);
    if (entry.isDirectory()) files.push(...(await listFiles(absolute)));
    else files.push(absolute);
  }
  return files;
}

async function readPublicReport() {
  return JSON.parse(await readFile(reportPath, "utf8"));
}

test("builds a complete static portfolio contract instead of a starter shell", async () => {
  const indexHtml = await readFile(path.join(distRoot, "index.html"), "utf8");
  const files = await listFiles(path.join(distRoot, "assets"));
  const bundleFiles = files.filter((file) => /\.(?:css|js)$/iu.test(file));
  const bundle = [
    indexHtml,
    ...(await Promise.all(bundleFiles.map((file) => readFile(file, "utf8")))),
  ].join("\n");

  assert.match(
    indexHtml,
    /<title>Reconstructing ES from 58,988,994 market events \| DBN\/ES<\/title>/u,
  );
  assert.match(indexHtml, /<div id="root">/u);
  assert.match(indexHtml, /<script type="module"/u);
  assert.ok(bundleFiles.some((file) => file.endsWith(".js")));
  assert.ok(bundleFiles.some((file) => file.endsWith(".css")));
  assert.match(bundle, /Reconstructing ES from/u);
  assert.match(bundle, /Live measured/u);
  assert.match(bundle, /Live derived/u);
  assert.match(bundle, /Synthetic teaching replay/u);
  assert.match(bundle, /data-story-step/u);
  for (const id of expectedSections) {
    assert.match(bundle, new RegExp(`["'\`]${id}["'\`]`, "u"));
  }
  assert.doesNotMatch(bundle, /Your site is taking shape|Building your site/u);
  assert.doesNotMatch(bundle, /codex-preview|react-loading-skeleton/u);
  assert.doesNotMatch(bundle, /site-creator-vinext-starter/u);
});

test("copies the versioned public report with the promoted evidence contract", async () => {
  const report = await readPublicReport();
  assert.equal(report.schema_version, 1);
  assert.equal(
    report.report_metadata.title,
    "Reconstructing ES from 58,988,994 market events",
  );
  assert.equal(report.dataset.total_records, 58_988_994);
  assert.equal(report.dataset.schemas.length, 4);
  assert.equal(report.validation.exact_matches, 23_961_616);
  assert.equal(report.validation.aligned_updates, 23_961_616);
  assert.equal(report.sweep.event_count, 11);
  assert.equal(report.benchmark.measured_configuration_count, 31);
  assert.equal(report.benchmark.unsupported_configuration_count, 1);
  assert.equal(report.synthetic.book_replay.evidence_status, "synthetic");
  assert.equal(report.synthetic.sweep_scenarios.evidence_status, "synthetic");
});

test("keeps totals, claims, provenance, and synthetic boundaries consistent", async () => {
  const report = await readPublicReport();
  const totals = report.dataset.schemas.reduce(
    (sum, schema) => ({
      records: sum.records + schema.records,
      compressedBytes: sum.compressedBytes + schema.compressed_bytes,
      decodedBytes: sum.decodedBytes + schema.decoded_bytes,
      costUsd: sum.costUsd + schema.cost_usd,
    }),
    { records: 0, compressedBytes: 0, decodedBytes: 0, costUsd: 0 },
  );

  assert.equal(totals.records, report.dataset.total_records);
  assert.equal(totals.compressedBytes, report.dataset.compressed_bytes);
  assert.equal(totals.decodedBytes, report.dataset.decoded_bytes);
  assert.ok(report.dataset.total_cost_usd >= totals.costUsd);
  assert.ok(Math.abs(totals.costUsd - report.dataset.total_cost_usd) < 1e-6);

  const allowedStatuses = new Set([
    "measured-live",
    "derived-live",
    "synthetic",
    "unsupported",
  ]);
  assert.ok(report.claims.length >= 9);
  for (const claim of report.claims) {
    assert.ok(allowedStatuses.has(claim.evidence_status), claim.id);
    assert.ok(claim.source_path.length > 0, claim.id);
    assert.ok(claim.source_locator.startsWith("/"), claim.id);
    assert.doesNotMatch(claim.source_path, privatePathPattern, claim.id);
    assert.equal(path.isAbsolute(claim.source_path), false, claim.id);
  }

  assert.equal(
    report.validation.exact_matches,
    report.validation.aligned_updates - report.validation.unmatched_mbp10_updates,
  );
  assert.equal(report.parity.record_count_match, true);
  assert.equal(report.parity.event_field_match, true);
  assert.equal(report.parity.event_count, report.sweep.event_count);
});

test("ships no paid payloads, credentials, private paths, or oversized files", async () => {
  const files = await listFiles(distRoot);
  assert.ok(files.length > 0);

  for (const file of files) {
    const relative = path.relative(distRoot, file).replaceAll("\\", "/");
    const metadata = await stat(file);
    assert.ok(metadata.size < 5 * 1024 * 1024, `${relative} exceeds 5 MiB`);
    assert.doesNotMatch(relative, /\.(?:dbn|zst|parquet|feather)$/iu);
    assert.doesNotMatch(relative, privatePathPattern);

    if (/\.(?:css|html|js|json|svg|txt|xml)$/iu.test(relative)) {
      const contents = await readFile(file, "utf8");
      assert.doesNotMatch(contents, privatePathPattern, relative);
      assert.doesNotMatch(contents, rawPayloadPattern, relative);
      assert.doesNotMatch(contents, credentialPattern, relative);
    }
  }
});
