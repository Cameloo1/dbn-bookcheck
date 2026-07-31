use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use dbn_es_core::decode_stats;
use serde_json::Value;

#[test]
fn synthetic_manifest_runs_end_to_end_and_pins_validation_rate() {
    let root = temp_root();
    if root.exists() {
        fs::remove_dir_all(&root).expect("remove stale synthetic fixture");
    }

    run(&["data", "sample", "--output-dir", path(&root)]);
    let manifest = root.join("manifest.json");
    let decode_report = root.join("decode-stats.json");
    run(&[
        "decode",
        "stats",
        "--manifest",
        path(&manifest),
        "--output",
        path(&decode_report),
    ]);

    let manifest_value: Value = read_json(&manifest);
    let entries = manifest_value["entries"]
        .as_array()
        .expect("manifest entries");
    assert_eq!(entries.len(), 4);
    for entry in entries {
        assert_eq!(entry["source"], "synthetic");
        let input = entry["path"].as_str().expect("entry path");
        let expected = entry["record_count"].as_u64().expect("record count");
        let stats = decode_stats(input).expect("decode generated DBN file");
        assert_eq!(stats.record_count, expected);
        assert_eq!(stats.timestamp_regression_count, 0);
    }

    let validation_md = root.join("book-validation.md");
    let validation_json = root.join("book-validation.json");
    run(&[
        "analyze",
        "validate",
        "--manifest",
        path(&manifest),
        "--output",
        path(&validation_md),
        "--json-output",
        path(&validation_json),
    ]);
    let validation: Value = read_json(&validation_json);
    assert_eq!(validation["aligned_updates"], 54);
    assert_eq!(validation["exact_matches"], 54);
    assert_eq!(validation["aligned_exact_match_pct"], 100.0);
    assert_eq!(validation["end_to_end_exact_match_pct"], 100.0);
    assert_eq!(validation["alignment_coverage_pct"], 100.0);

    let sweeps = root.join("sweeps.jsonl");
    let summary = root.join("sweep-summary.json");
    let sweep_report = root.join("sweep-report.md");
    let sweep_config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/sweep.json");
    run(&[
        "analyze",
        "sweeps",
        "--manifest",
        path(&manifest),
        "--config",
        path(&sweep_config),
        "--output",
        path(&sweeps),
        "--summary",
        path(&summary),
        "--report",
        path(&sweep_report),
    ]);
    let sweep_summary: Value = read_json(&summary);
    assert_eq!(sweep_summary["mbo_records_scanned"], 55);
    assert_eq!(sweep_summary["event_count"], 1);
    assert_eq!(
        fs::read_to_string(&sweeps)
            .expect("sweep output")
            .lines()
            .count(),
        1
    );

    fs::remove_dir_all(root).expect("remove synthetic fixture");
}

fn run(args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_dbn-es-bench"))
        .args(args)
        .output()
        .expect("run dbn-es-bench");
    assert!(
        output.status.success(),
        "dbn-es-bench {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn read_json(path: &Path) -> Value {
    serde_json::from_reader(fs::File::open(path).expect("open JSON")).expect("parse JSON")
}

fn path(path: &Path) -> &str {
    path.to_str().expect("UTF-8 test path")
}

fn temp_root() -> PathBuf {
    std::env::temp_dir().join(format!("dbn-es-pipeline-{}", std::process::id()))
}
