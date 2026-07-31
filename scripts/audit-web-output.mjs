#!/usr/bin/env node

import { readdir, readFile, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const relativeTarget = process.argv[2] ?? "web/public/data/report.v1.json";

const textExtensions = new Set([
  ".css",
  ".csv",
  ".html",
  ".js",
  ".json",
  ".map",
  ".mjs",
  ".svg",
  ".txt",
  ".webmanifest",
  ".xml"
]);

const forbiddenContent = [
  { label: "private Windows path", pattern: /\b[A-Za-z]:[\\/](?:Users|Documents and Settings)[\\/]/iu },
  { label: "private Unix home path", pattern: /\/(?:home|Users)\/[^/\s"'<>]+/u },
  { label: "file URL", pattern: /file:\/\//iu },
  { label: "private market-data API endpoint", pattern: /https?:\/\/[^/\s"'<>]*databento\.[^/\s"'<>]+/iu },
  { label: "private key material", pattern: /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/u },
  { label: "OpenAI-style secret", pattern: /\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}\b/u },
  { label: "Databento-style secret", pattern: /\bdb-[A-Za-z0-9_-]{20,}\b/u },
  { label: "AWS access key", pattern: /\bAKIA[A-Z0-9]{16}\b/u }
];

const forbiddenJsonKeys = new Set([
  "api_key",
  "checksum",
  "local_path",
  "output_dir",
  "replay_token",
  "request_id",
  "secret",
  "token"
]);

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function safeTarget(relativePath) {
  assert(typeof relativePath === "string" && relativePath.length > 0, "target path is required");
  assert(!path.isAbsolute(relativePath), "target must be repository-relative");
  assert(!relativePath.split(/[\\/]/u).includes(".."), "target cannot traverse outside the repository");
  return path.join(repoRoot, relativePath);
}

async function collectFiles(targetPath) {
  const metadata = await stat(targetPath);
  if (metadata.isFile()) {
    return [targetPath];
  }
  assert(metadata.isDirectory(), "audit target must be a file or directory");
  const entries = await readdir(targetPath, { withFileTypes: true });
  const nested = await Promise.all(
    entries
      .sort((left, right) => left.name.localeCompare(right.name))
      .map((entry) => collectFiles(path.join(targetPath, entry.name)))
  );
  return nested.flat();
}

function inspectJsonKeys(value, relativeFile, pointer = "") {
  if (Array.isArray(value)) {
    value.forEach((item, index) => inspectJsonKeys(item, relativeFile, `${pointer}/${index}`));
    return;
  }
  if (!value || typeof value !== "object") {
    return;
  }
  for (const [key, item] of Object.entries(value)) {
    assert(!forbiddenJsonKeys.has(key.toLowerCase()), `${relativeFile}${pointer}: forbidden public key "${key}"`);
    inspectJsonKeys(item, relativeFile, `${pointer}/${key}`);
  }
}

const targetPath = safeTarget(relativeTarget);
const files = await collectFiles(targetPath);
assert(files.length > 0, "audit target is empty");

let textFileCount = 0;
for (const file of files) {
  const relativeFile = path.relative(repoRoot, file).replaceAll("\\", "/");
  assert(!/\.dbn(?:\.zst)?$/iu.test(file), `${relativeFile}: paid DBN payload cannot ship`);
  assert(
    !/(?:^|\/)(?:data\/(?:manifest|quote|spend-state)|spend-ledger|quote-response)\.json$/iu.test(
      relativeFile
    ),
    `${relativeFile}: private acquisition artifact cannot ship`
  );

  const extension = path.extname(file).toLowerCase();
  if (!textExtensions.has(extension)) {
    continue;
  }
  textFileCount += 1;
  const contents = await readFile(file, "utf8");
  for (const rule of forbiddenContent) {
    assert(!rule.pattern.test(contents), `${relativeFile}: ${rule.label} detected`);
  }
  if (extension === ".json" || extension === ".webmanifest") {
    inspectJsonKeys(JSON.parse(contents), relativeFile);
  }
}

process.stdout.write(
  `web output audit passed: ${relativeTarget} (${files.length} files, ${textFileCount} text files inspected)\n`
);
