#!/usr/bin/env node

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const treeOnly = process.argv.includes("--tree-only") || !existsSync(join(root, ".git"));
const git = (...args) => execFileSync("git", ["-c", `safe.directory=${root.replaceAll("\\", "/")}`, ...args], { cwd: root, encoding: "utf8" }).trim();
const read = (path) => readFileSync(join(root, path), "utf8");
const repositoryUrl = "https://github.com/Cameloo1/dbn-bookcheck";
const npmRepositoryUrl = `git+${repositoryUrl}.git`;
const failures = [];
const pass = [];
const check = (condition, message) => {
  if (condition) pass.push(message);
  else failures.push(message);
};

const excludedTreeDirectories = new Set([
  ".git",
  ".next",
  ".vinext",
  ".wrangler",
  "coverage",
  "dist",
  "node_modules",
  "out",
  "outputs",
  "playwright-report",
  "target",
  "test-results",
]);
const collectCurrentFiles = (directory = root) => {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.isDirectory() && excludedTreeDirectories.has(entry.name)) continue;
    const absolutePath = join(directory, entry.name);
    const path = relative(root, absolutePath).replaceAll("\\", "/");
    if (path.startsWith("data/") && path !== "data/.gitkeep") continue;
    if (entry.isDirectory()) files.push(...collectCurrentFiles(absolutePath));
    else if (entry.isFile()) files.push(path);
  }
  return files.sort();
};
const currentFiles = treeOnly ? collectCurrentFiles() : [];
const readTextIfApplicable = (path) => {
  const content = readFileSync(join(root, path));
  return content.includes(0) ? null : content.toString("utf8");
};
const commits = treeOnly ? [] : git("rev-list", "--all").split(/\r?\n/u).filter(Boolean);
const secretPattern = [
  "db-[A-Za-z0-9_-]{20,}",
  "-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----",
  "ghp_[A-Za-z0-9]{20,}",
  "github_pat_[A-Za-z0-9_]{20,}",
  "sk-[A-Za-z0-9_-]{20,}",
  "AKIA[0-9A-Z]{16}",
  "npm_[A-Za-z0-9]{20,}",
].join("|");
const secretLocations = [];
const sensitiveFiles = [];
if (treeOnly) {
  const secretRegex = new RegExp(secretPattern, "u");
  for (const path of currentFiles) {
    if (/(^|\/)(\.env($|\.)|[^/]+\.(pem|p12|pfx|key)$|id_(rsa|ed25519)$)/iu.test(path)) sensitiveFiles.push(path);
    const content = readTextIfApplicable(path);
    if (content && secretRegex.test(content)) secretLocations.push(path);
  }
} else {
  for (const commit of commits) {
    const grep = spawnSync(
      "git",
      ["-c", `safe.directory=${root.replaceAll("\\", "/")}`, "grep", "-I", "-l", "-E", secretPattern, commit, "--", "."],
      { cwd: root, encoding: "utf8" },
    );
    if (grep.status === 0) {
      for (const path of grep.stdout.trim().split(/\r?\n/u).filter(Boolean)) secretLocations.push(`${commit}:${path}`);
    } else if (grep.status !== 1) {
      failures.push(`git grep failed for commit ${commit}`);
    }
    for (const path of git("ls-tree", "-r", "--name-only", commit).split(/\r?\n/u)) {
      if (/(^|\/)(\.env($|\.)|[^/]+\.(pem|p12|pfx|key)$|id_(rsa|ed25519)$)/iu.test(path)) sensitiveFiles.push(`${commit}:${path}`);
    }
  }
}
const auditScope = treeOnly ? `${currentFiles.length} source-tree files` : `${commits.length} reachable commits`;
check(secretLocations.length === 0, `no secret-shaped content across ${auditScope}`);
check(sensitiveFiles.length === 0, `no sensitive filenames across ${auditScope}`);

const tracked = treeOnly ? currentFiles : git("ls-files").split(/\r?\n/u).filter(Boolean);
const prohibitedTracked = tracked.filter((path) =>
  /(^|\/)(target|node_modules|out|coverage)(\/|$)|(^|\/)\.env($|\.)|\.(dbn(?:\.zst)?|node|log|profraw)$/iu.test(path) ||
  /(^|\/)package-lock\.json$/u.test(path) ||
  /^(?:bench\/(?:machine|results)\.json|config\/live-session\.json|data\/spend-log\.md|evidence\/public\/[^/]+\.json)$/u.test(path) ||
  (path.startsWith("data/") && path !== "data/.gitkeep"),
);
check(
  prohibitedTracked.length === 0,
  prohibitedTracked.length === 0
    ? "no generated payloads, DBN files, native binaries, logs, dependency trees, or environment files are tracked"
    : `generated or private artifact paths are present in the audited source set: ${prohibitedTracked.join(", ")}`,
);

const productionLibraries = [
  "crates/dbn-es-core/src/decode.rs",
  "crates/dbn-es-core/src/order_book.rs",
  "crates/dbn-es-core/src/sweep.rs",
  "crates/dbn-es-node/src/lib.rs",
];
const forbidden = [];
for (const path of productionLibraries) {
  const production = read(path).split("#[cfg(test)]")[0];
  if (/\b(?:unwrap|expect)\s*\(|\b(?:todo|unimplemented|dbg)!\s*\(/u.test(production)) forbidden.push(path);
}
check(forbidden.length === 0, "no unwrap, expect, todo, unimplemented, or dbg invocation occurs in production library code");

if (treeOnly) {
  const placeholderFiles = currentFiles.filter((path) => /github\.com\/example|example\.com/iu.test(readTextIfApplicable(path) ?? ""));
  check(placeholderFiles.length === 0, "no placeholder repository URL remains in the current tree");
} else {
  const placeholderScan = spawnSync(
    "git",
    ["-c", `safe.directory=${root.replaceAll("\\", "/")}`, "grep", "-I", "-l", "-E", "github\\.com/example|example\\.com", "--", "."],
    { cwd: root, encoding: "utf8" },
  );
  check(placeholderScan.status === 1, "no placeholder repository URL remains in the current tree");
}

check(read("Cargo.toml").includes(`repository = "${repositoryUrl}"`), "Cargo workspace names the verified GitHub repository");
for (const path of tracked.filter((entry) => /^crates\/[^/]+\/Cargo\.toml$/u.test(entry))) {
  check(read(path).includes("repository.workspace = true"), `${path} inherits the verified repository identity`);
}

for (const path of ["node/package.json", ...tracked.filter((entry) => /^node\/npm\/[^/]+\/package\.json$/u.test(entry))]) {
  try {
    const manifest = JSON.parse(read(path));
    check(manifest.license === "Apache-2.0", `${path} is valid JSON with the intended license`);
    check(manifest.repository?.url === npmRepositoryUrl, `${path} names the verified GitHub repository`);
  } catch (error) {
    failures.push(`${path} is invalid JSON: ${error.message}`);
  }
}

const publicReport = JSON.parse(read("web/public/data/report.v1.json"));
const benchmark = publicReport.benchmark;
const validation = read("docs/book-validation.md");
const sweep = read("docs/sweep-detection.md");
const readme = read("README.md");
const summary = read("docs/summary.md");
const resultsHtml = read("docs/results.html");
const headline = benchmark.results.find((row) => row.schema === "mbo" && row.compression === "zstd" && row.access === "streaming" && row.concurrency === "single_thread");
check(publicReport.schema_version === 1 && benchmark.results.length === 31 && benchmark.unsupported_configurations.length === 1, "promoted public report exposes all 31 measured rows and one bounded unsupported row");
check(Boolean(headline), "headline benchmark configuration exists without selecting a hidden row");
if (headline) {
  const rate = Math.round(headline.messages_per_second_median).toLocaleString("en-US");
  for (const [path, document] of [["README.md", readme], ["docs/summary.md", summary], ["docs/results.html", resultsHtml]]) {
    check(document.includes(rate), `${path} headline throughput traces to the promoted public report`);
  }
}
const exact = validation.match(/\| Exact price-and-size matches \| (\d+) \|/u)?.[1];
check(Boolean(exact), "book validation contains a generated exact-match denominator");
if (exact) {
  const formatted = Number(exact).toLocaleString("en-US");
  for (const [path, document] of [["README.md", readme], ["docs/summary.md", summary], ["docs/results.html", resultsHtml]]) {
    check(document.includes(formatted), `${path} validation count traces to docs/book-validation.md`);
  }
}
check(/Provenance: `live`/u.test(sweep) && /not a trading signal/u.test(sweep), "sweep report surfaces live provenance and the heuristic boundary");
check(/synthetic/u.test(read("docs/DEMO.md")) && /not market data/u.test(read("docs/DEMO.md")), "demo documentation labels synthetic provenance explicitly");
check(!readme.includes("{{"), "generated README contains no unresolved template tokens");
check(readme.includes(repositoryUrl), "generated README names the verified GitHub repository");
check(!/(?:https?:)?\/\//iu.test(resultsHtml) && !/<script\b/iu.test(resultsHtml), "offline results page contains no remote URL or script tag");

if (failures.length > 0) {
  console.error(`repository audit failed (${failures.length} checks):`);
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

const completionScope = treeOnly
  ? `${currentFiles.length} current source-tree files scanned; Git history intentionally not inspected`
  : `${commits.length} commits scanned`;
console.log(`repository audit passed: ${pass.length} checks, ${completionScope}`);
for (const item of pass) console.log(`- ${item}`);
