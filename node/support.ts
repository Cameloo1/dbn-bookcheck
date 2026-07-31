import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import type { JsSweepConfig } from "./native.js";

export interface ManifestEntry {
  path: string;
  schema: string;
  source: "live" | "fixture" | "synthetic";
}

interface Manifest {
  source: "live" | "fixture" | "synthetic";
  entries: ManifestEntry[];
}

interface FileSweepConfig {
  lookback_trades: number;
  threshold_ticks: number;
  reversion_window_ms: number;
  tick_size: number;
}

export const packageRoot = resolve(__dirname, "..");
export const workspaceRoot = resolve(packageRoot, "..");

export function loadMboEntry(manifestPath?: string): ManifestEntry {
  const resolvedManifest = resolve(
    manifestPath ?? resolve(workspaceRoot, "data", "manifest.json"),
  );
  const manifest = JSON.parse(readFileSync(resolvedManifest, "utf8")) as Manifest;
  const entry = manifest.entries.find((candidate) => candidate.schema === "mbo");
  if (entry === undefined) {
    throw new Error(`no MBO entry in ${resolvedManifest}`);
  }
  console.log(`data provenance: ${entry.source}`);
  return { ...entry, path: resolve(workspaceRoot, entry.path) };
}

export function json(value: unknown): string {
  return JSON.stringify(
    value,
    (_key, item: unknown) => (typeof item === "bigint" ? item.toString() : item),
  );
}

export function loadSweepConfig(): JsSweepConfig {
  const value = JSON.parse(
    readFileSync(resolve(workspaceRoot, "config", "sweep.json"), "utf8"),
  ) as FileSweepConfig;
  return {
    lookbackTrades: value.lookback_trades,
    thresholdTicks: value.threshold_ticks,
    reversionWindowMs: value.reversion_window_ms,
    tickSize: value.tick_size,
  };
}
