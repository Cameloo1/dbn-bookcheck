#!/usr/bin/env node

import { readFileSync } from "node:fs";

const path = process.argv[2] ?? "docs/results.html";
const html = readFileSync(path, "utf8");
const failures = [];
const requireText = (pattern, message) => {
  if (!pattern.test(html)) failures.push(message);
};

requireText(/^<!doctype html>/iu, "missing HTML5 doctype");
requireText(/<html lang="en">/u, "missing document language");
requireText(/<meta charset="utf-8">/u, "missing UTF-8 declaration");
requireText(/<meta name="viewport"/u, "missing responsive viewport");
requireText(/<svg[^>]+role="img"[^>]+aria-label="Decoded throughput by schema"/u, "missing accessible throughput chart");
requireText(/<svg[^>]+role="img"[^>]+aria-label="Book reconstruction discrepancy histogram"/u, "missing accessible discrepancy chart");
requireText(/<table>/u, "missing tabular fallback");
requireText(/No external scripts, fonts, telemetry, or network requests/u, "missing offline boundary statement");

if (/<script\b/iu.test(html)) failures.push("script tags are not permitted in the offline artifact");
if (/(?:https?:)?\/\//iu.test(html)) failures.push("remote or protocol-relative URL found");
if (/\b(?:src|href)\s*=/iu.test(html)) failures.push("external asset reference found");
if (/@(?:import|font-face)\b/iu.test(html)) failures.push("externalizable CSS construct found");

const openingSvg = (html.match(/<svg\b/gu) ?? []).length;
const closingSvg = (html.match(/<\/svg>/gu) ?? []).length;
if (openingSvg !== 2 || closingSvg !== 2) failures.push("expected two balanced inline SVG charts");

const openingTable = (html.match(/<table>/gu) ?? []).length;
const closingTable = (html.match(/<\/table>/gu) ?? []).length;
if (openingTable !== 2 || closingTable !== 2) failures.push("expected two balanced fallback tables");

if (failures.length > 0) {
  for (const failure of failures) console.error(`results HTML: ${failure}`);
  process.exit(1);
}

console.log(`results HTML validation passed: ${path} (self-contained, two accessible inline SVG charts)`);
