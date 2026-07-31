export type EvidenceStatus =
  | "measured-live"
  | "derived-live"
  | "synthetic"
  | "unsupported";

export interface PublicClaim {
  id: string;
  label: string;
  value: number | string;
  display: string;
  unit: string;
  evidence_status: EvidenceStatus;
  source_path: string;
  source_locator: string;
  method_note: string;
  limitations: string;
}

export interface DatasetSchema {
  name: string;
  label: string;
  records: number;
  compressed_bytes: number;
  decoded_bytes: number;
  cost_usd: number;
  used_for: string;
}

export interface BenchmarkCapability {
  feature: string;
  status: string;
  detail: string;
}

export interface BenchmarkRow {
  schema: string;
  compression: "zstd" | "none";
  access: "streaming" | "fully_buffered_input";
  concurrency: "single_thread" | "parallel_independent_streams";
  threads: number;
  measured_runs: number;
  warmup_runs_discarded: number;
  messages_per_run: number;
  wire_bytes_per_run: number;
  decoded_bytes_per_run: number;
  elapsed_seconds_median: number;
  elapsed_seconds_p95: number;
  elapsed_seconds_stddev: number;
  messages_per_second_median: number;
  messages_per_second_p95: number;
  messages_per_second_stddev: number;
  wire_mib_per_second_median: number;
  decoded_mib_per_second_median: number;
  nanoseconds_per_message_median: number;
  peak_rss_mib_median: number;
  peak_rss_mib_max: number;
  allocation_count: number | null;
}

export interface UnsupportedBenchmark {
  schema: string;
  compression: string;
  access: string;
  concurrency: string;
  reason: string;
}

export interface PublicReport {
  schema_version: number;
  dataset: {
    name: string;
    symbol: string;
    start: string;
    end: string;
    session_hours: number;
    total_records: number;
    compressed_bytes: number;
    decoded_bytes: number;
    total_cost_usd: number;
    schemas: DatasetSchema[];
  };
  claims: PublicClaim[];
  validation: {
    mbo_records_scanned: number;
    mbp10_records_scanned: number;
    mbo_records_withheld_before_baseline: number;
    mbp10_updates_before_valid_baseline: number;
    aligned_updates: number;
    unmatched_mbo_observations: number;
    unmatched_mbp10_updates: number;
    exact_matches: number;
    end_to_end_exact_match_pct: number;
    methodology: string;
  };
  sweep: {
    parameters: {
      lookback_trades: number;
      threshold_ticks: number;
      reversion_window_ms: number;
      tick_size_points: number;
    };
    event_count: number;
    above_high_count: number;
    below_low_count: number;
    mbo_records_scanned: number;
  };
  parity: {
    mbo_records: number;
    event_count: number;
  };
  benchmark: {
    generated_at: string;
    methodology: string;
    capabilities: BenchmarkCapability[];
    results: BenchmarkRow[];
    unsupported_configurations: UnsupportedBenchmark[];
  };
  limitations: string[];
}
