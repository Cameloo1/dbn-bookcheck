use std::{
    cmp::Ordering,
    fs::{self, File},
    hint::black_box,
    io::{self, BufReader, BufWriter, Cursor, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    thread,
    time::Instant,
};

use clap::{Parser, Subcommand, ValueEnum};
use dbn::{
    Record, VersionUpgradePolicy,
    decode::{DecodeRecordRef, DynDecoder},
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MIB: f64 = 1024.0 * 1024.0;

#[derive(Debug, Parser)]
#[command(name = "dbn-es-benchmark", version, about)]
struct Cli {
    #[command(subcommand)]
    command: BenchmarkCommand,
}

#[derive(Debug, Subcommand)]
enum BenchmarkCommand {
    /// Materialize raw inputs, run isolated samples, and aggregate stable results.
    Run {
        #[arg(long, default_value = "data/manifest.json")]
        manifest: PathBuf,
        #[arg(long, default_value = "data/uncompressed")]
        uncompressed_dir: PathBuf,
        #[arg(long, default_value = "bench/results.json")]
        output: PathBuf,
        #[arg(long, default_value_t = 5)]
        runs: usize,
        #[arg(long, default_value_t = 1)]
        warmups: usize,
        #[arg(long, default_value_t = 4)]
        parallel_streams: usize,
        #[arg(long, default_value_t = 16 * 1024 * 1024 * 1024_u64)]
        max_buffered_bytes: u64,
        /// Runs one small representative configuration for CI wiring checks.
        #[arg(long)]
        smoke: bool,
    },
    /// Internal isolated measurement. Invoke `run` instead.
    Once {
        #[arg(long)]
        path: PathBuf,
        #[arg(long, value_enum)]
        access: AccessMode,
        #[arg(long)]
        threads: usize,
        #[arg(long)]
        expected_records: u64,
        #[arg(long)]
        wire_bytes: u64,
        #[arg(long)]
        decoded_bytes: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum AccessMode {
    Streaming,
    FullyBufferedInput,
}

impl AccessMode {
    const ALL: [Self; 2] = [Self::Streaming, Self::FullyBufferedInput];

    const fn as_arg(self) -> &'static str {
        match self {
            Self::Streaming => "streaming",
            Self::FullyBufferedInput => "fully-buffered-input",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CompressionMode {
    Zstd,
    None,
}

#[derive(Debug, thiserror::Error)]
enum BenchmarkError {
    #[error("I/O error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("DBN decode failed for {path}: {source}")]
    Dbn {
        path: PathBuf,
        #[source]
        source: dbn::Error,
    },
    #[error("invalid benchmark configuration: {0}")]
    Config(String),
    #[error("benchmark child failed: {0}")]
    Child(String),
    #[error("worker thread panicked")]
    WorkerPanic,
    #[error("failed to format timestamp: {0}")]
    Timestamp(#[from] time::error::Format),
}

#[derive(Debug, Deserialize)]
struct Manifest {
    source: String,
    entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestEntry {
    path: PathBuf,
    schema: String,
    compressed_bytes: u64,
    uncompressed_bytes: u64,
    record_count: u64,
}

#[derive(Debug, Clone)]
struct InputVariant {
    schema: String,
    path: PathBuf,
    compression: CompressionMode,
    wire_bytes: u64,
    decoded_bytes: u64,
    record_count: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct SingleRun {
    elapsed_seconds: f64,
    messages: u64,
    wire_bytes: u64,
    decoded_bytes: u64,
    checksum: u64,
    peak_rss_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
struct Capability {
    feature: &'static str,
    status: &'static str,
    detail: &'static str,
}

#[derive(Debug, Serialize)]
struct UnsupportedConfiguration {
    schema: String,
    compression: CompressionMode,
    access: AccessMode,
    concurrency: &'static str,
    reason: String,
}

#[derive(Debug, Serialize)]
struct BenchmarkResult {
    schema: String,
    compression: CompressionMode,
    access: AccessMode,
    concurrency: &'static str,
    threads: usize,
    measured_runs: usize,
    warmup_runs_discarded: usize,
    messages_per_run: u64,
    wire_bytes_per_run: u64,
    decoded_bytes_per_run: u64,
    elapsed_seconds_median: f64,
    elapsed_seconds_p95: f64,
    elapsed_seconds_stddev: f64,
    messages_per_second_median: f64,
    messages_per_second_p95: f64,
    messages_per_second_stddev: f64,
    wire_mib_per_second_median: f64,
    decoded_mib_per_second_median: f64,
    nanoseconds_per_message_median: f64,
    peak_rss_mib_median: Option<f64>,
    peak_rss_mib_max: Option<f64>,
    allocation_count: Option<u64>,
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    version: u32,
    generated_at: String,
    source: String,
    machine_file: &'static str,
    profile: &'static str,
    cache_condition: &'static str,
    methodology: &'static str,
    capabilities: Vec<Capability>,
    unsupported_configurations: Vec<UnsupportedConfiguration>,
    results: Vec<BenchmarkResult>,
}

fn main() -> Result<(), BenchmarkError> {
    match Cli::parse().command {
        BenchmarkCommand::Run {
            manifest,
            uncompressed_dir,
            output,
            runs,
            warmups,
            parallel_streams,
            max_buffered_bytes,
            smoke,
        } => run_matrix(&RunOptions {
            manifest_path: manifest,
            uncompressed_dir,
            output_path: output,
            runs,
            warmups,
            parallel_streams,
            max_buffered_bytes,
            smoke,
        }),
        BenchmarkCommand::Once {
            path,
            access,
            threads,
            expected_records,
            wire_bytes,
            decoded_bytes,
        } => {
            let result = measure_once(
                &path,
                access,
                threads,
                expected_records,
                wire_bytes,
                decoded_bytes,
            )?;
            println!(
                "{}",
                serde_json::to_string(&result).map_err(|source| BenchmarkError::Json {
                    path: PathBuf::from("<stdout>"),
                    source,
                })?
            );
            Ok(())
        }
    }
}

struct RunOptions {
    manifest_path: PathBuf,
    uncompressed_dir: PathBuf,
    output_path: PathBuf,
    runs: usize,
    warmups: usize,
    parallel_streams: usize,
    max_buffered_bytes: u64,
    smoke: bool,
}

#[allow(clippy::too_many_lines)]
fn run_matrix(options: &RunOptions) -> Result<(), BenchmarkError> {
    validate_run_options(options)?;
    let manifest = read_json::<Manifest>(&options.manifest_path)?;
    if manifest.entries.is_empty() {
        return Err(BenchmarkError::Config("manifest is empty".to_owned()));
    }
    let entries: Vec<ManifestEntry> = if options.smoke {
        manifest
            .entries
            .into_iter()
            .filter(|entry| entry.schema == "ohlcv-1m")
            .collect()
    } else {
        manifest.entries
    };
    if entries.is_empty() {
        return Err(BenchmarkError::Config(
            "smoke mode requires an ohlcv-1m manifest entry".to_owned(),
        ));
    }
    let inputs = materialize_inputs(&entries, &options.uncompressed_dir, options.smoke)?;
    let executable = std::env::current_exe().map_err(|source| BenchmarkError::Io {
        path: PathBuf::from("<current-executable>"),
        source,
    })?;
    let mut results = Vec::new();
    let mut unsupported = Vec::new();

    for input in &inputs {
        for access in AccessMode::ALL {
            for (concurrency, threads) in [
                ("single_thread", 1_usize),
                ("parallel_independent_streams", options.parallel_streams),
            ] {
                let buffered_bytes = input
                    .wire_bytes
                    .checked_mul(u64::try_from(threads).map_err(|_| {
                        BenchmarkError::Config("thread count does not fit u64".to_owned())
                    })?)
                    .ok_or_else(|| {
                        BenchmarkError::Config("buffered byte plan overflows u64".to_owned())
                    })?;
                if access == AccessMode::FullyBufferedInput
                    && buffered_bytes > options.max_buffered_bytes
                {
                    unsupported.push(UnsupportedConfiguration {
                        schema: input.schema.clone(),
                        compression: input.compression,
                        access,
                        concurrency,
                        reason: format!(
                            "planned input buffers total {buffered_bytes} bytes, above the configured {}-byte safety gate",
                            options.max_buffered_bytes
                        ),
                    });
                    continue;
                }
                eprintln!(
                    "measuring schema={} compression={:?} access={access:?} concurrency={concurrency}",
                    input.schema, input.compression
                );
                for _ in 0..options.warmups {
                    let _ = run_child(&executable, input, access, threads)?;
                }
                let mut samples = Vec::with_capacity(options.runs);
                for _ in 0..options.runs {
                    samples.push(run_child(&executable, input, access, threads)?);
                }
                results.push(summarize_configuration(
                    input,
                    access,
                    concurrency,
                    threads,
                    options.warmups,
                    &samples,
                )?);
            }
        }
    }

    let report = BenchmarkReport {
        version: 1,
        generated_at: OffsetDateTime::now_utc().format(&Rfc3339)?,
        source: manifest.source,
        machine_file: "bench/machine.json",
        profile: "release",
        cache_condition: "warm",
        methodology: "Each configuration runs in a fresh child process. One warmup is discarded and at least five measured runs are aggregated with median, nearest-rank p95, and population standard deviation. Timings include file reads, decompression, DBN parsing, record traversal, thread startup, and joins. Parallel rows decode independent identical streams and report aggregate throughput.",
        capabilities: vec![
            Capability {
                feature: "warm_page_cache",
                status: "measured",
                detail: "Every configuration is warmed explicitly before recorded child runs.",
            },
            Capability {
                feature: "cold_page_cache",
                status: "unsupported",
                detail: "The managed WSL run has no safe, isolated OS page-cache eviction primitive; no cold numbers are fabricated.",
            },
            Capability {
                feature: "peak_resident_set_size",
                status: "measured",
                detail: "Each child reads its own Linux /proc/self/status VmHWM value.",
            },
            Capability {
                feature: "allocation_count",
                status: "not_instrumentable",
                detail: "The forbid-unsafe workspace does not install a global allocator shim; results use null rather than zero.",
            },
            Capability {
                feature: "parallel_decode",
                status: "measured",
                detail: "Parallel rows run independent streams, because one DBN/Zstd stream is sequential and is not falsely presented as splittable.",
            },
        ],
        unsupported_configurations: unsupported,
        results,
    };
    write_json_atomic(&options.output_path, &report)?;
    println!(
        "wrote {} measured configurations and {} unsupported configurations to {}",
        report.results.len(),
        report.unsupported_configurations.len(),
        options.output_path.display()
    );
    Ok(())
}

fn validate_run_options(options: &RunOptions) -> Result<(), BenchmarkError> {
    if options.runs == 0 || options.warmups == 0 {
        return Err(BenchmarkError::Config(
            "runs and warmups must both be positive".to_owned(),
        ));
    }
    if !options.smoke && options.runs < 5 {
        return Err(BenchmarkError::Config(
            "full benchmark requires at least five measured runs".to_owned(),
        ));
    }
    if options.parallel_streams < 2 {
        return Err(BenchmarkError::Config(
            "parallel_streams must be at least two".to_owned(),
        ));
    }
    Ok(())
}

fn materialize_inputs(
    entries: &[ManifestEntry],
    uncompressed_dir: &Path,
    smoke: bool,
) -> Result<Vec<InputVariant>, BenchmarkError> {
    let mut inputs = Vec::with_capacity(entries.len() * 2);
    for entry in entries {
        inputs.push(InputVariant {
            schema: entry.schema.clone(),
            path: entry.path.clone(),
            compression: CompressionMode::Zstd,
            wire_bytes: entry.compressed_bytes,
            decoded_bytes: entry.uncompressed_bytes,
            record_count: entry.record_count,
        });
        if !smoke {
            let raw_path = raw_path_for(entry, uncompressed_dir)?;
            ensure_uncompressed(entry, &raw_path)?;
            inputs.push(InputVariant {
                schema: entry.schema.clone(),
                path: raw_path,
                compression: CompressionMode::None,
                wire_bytes: entry.uncompressed_bytes,
                decoded_bytes: entry.uncompressed_bytes,
                record_count: entry.record_count,
            });
        }
    }
    Ok(inputs)
}

fn raw_path_for(entry: &ManifestEntry, directory: &Path) -> Result<PathBuf, BenchmarkError> {
    let file_name = entry
        .path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            BenchmarkError::Config(format!("invalid input path {}", entry.path.display()))
        })?;
    let raw_name = file_name
        .strip_suffix(".zst")
        .ok_or_else(|| BenchmarkError::Config(format!("expected .zst input: {file_name}")))?;
    Ok(directory.join(raw_name))
}

fn ensure_uncompressed(entry: &ManifestEntry, destination: &Path) -> Result<(), BenchmarkError> {
    if destination.is_file() {
        let actual = fs::metadata(destination)
            .map_err(|source| BenchmarkError::Io {
                path: destination.to_path_buf(),
                source,
            })?
            .len();
        if actual == entry.uncompressed_bytes {
            return Ok(());
        }
        return Err(BenchmarkError::Config(format!(
            "existing raw file {} has {actual} bytes, expected {}; remove or quarantine it before retrying",
            destination.display(),
            entry.uncompressed_bytes
        )));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|source| BenchmarkError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let part = part_path(destination);
    eprintln!(
        "materializing {} -> {}",
        entry.path.display(),
        destination.display()
    );
    let input = File::open(&entry.path).map_err(|source| BenchmarkError::Io {
        path: entry.path.clone(),
        source,
    })?;
    let mut decoder =
        zstd::stream::read::Decoder::new(BufReader::new(input)).map_err(|source| {
            BenchmarkError::Io {
                path: entry.path.clone(),
                source,
            }
        })?;
    let output = File::create(&part).map_err(|source| BenchmarkError::Io {
        path: part.clone(),
        source,
    })?;
    let mut writer = BufWriter::new(output);
    let bytes = io::copy(&mut decoder, &mut writer).map_err(|source| BenchmarkError::Io {
        path: part.clone(),
        source,
    })?;
    writer.flush().map_err(|source| BenchmarkError::Io {
        path: part.clone(),
        source,
    })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| BenchmarkError::Io {
            path: part.clone(),
            source,
        })?;
    if bytes != entry.uncompressed_bytes {
        return Err(BenchmarkError::Config(format!(
            "materialized {} bytes for {}, expected {}",
            bytes,
            destination.display(),
            entry.uncompressed_bytes
        )));
    }
    fs::rename(&part, destination).map_err(|source| BenchmarkError::Io {
        path: destination.to_path_buf(),
        source,
    })
}

fn run_child(
    executable: &Path,
    input: &InputVariant,
    access: AccessMode,
    threads: usize,
) -> Result<SingleRun, BenchmarkError> {
    let output = Command::new(executable)
        .arg("once")
        .arg("--path")
        .arg(&input.path)
        .arg("--access")
        .arg(access.as_arg())
        .arg("--threads")
        .arg(threads.to_string())
        .arg("--expected-records")
        .arg(input.record_count.to_string())
        .arg("--wire-bytes")
        .arg(input.wire_bytes.to_string())
        .arg("--decoded-bytes")
        .arg(input.decoded_bytes.to_string())
        .output()
        .map_err(|source| BenchmarkError::Io {
            path: executable.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Err(BenchmarkError::Child(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|source| BenchmarkError::Json {
        path: PathBuf::from("<benchmark-child-stdout>"),
        source,
    })
}

fn measure_once(
    path: &Path,
    access: AccessMode,
    threads: usize,
    expected_records: u64,
    wire_bytes: u64,
    decoded_bytes: u64,
) -> Result<SingleRun, BenchmarkError> {
    if threads == 0 {
        return Err(BenchmarkError::Config(
            "threads must be positive".to_owned(),
        ));
    }
    let started = Instant::now();
    let mut handles = Vec::with_capacity(threads);
    for _ in 0..threads {
        let path = path.to_path_buf();
        handles.push(thread::spawn(move || decode_path(&path, access)));
    }
    let mut messages = 0_u64;
    let mut checksum = 0_u64;
    for handle in handles {
        let (count, worker_checksum) = handle.join().map_err(|_| BenchmarkError::WorkerPanic)??;
        if count != expected_records {
            return Err(BenchmarkError::Config(format!(
                "decoded {count} records from {}, expected {expected_records}",
                path.display()
            )));
        }
        messages = messages
            .checked_add(count)
            .ok_or_else(|| BenchmarkError::Config("message total overflows u64".to_owned()))?;
        checksum ^= worker_checksum;
    }
    let elapsed_seconds = started.elapsed().as_secs_f64();
    let multiplier = u64::try_from(threads)
        .map_err(|_| BenchmarkError::Config("thread count does not fit u64".to_owned()))?;
    Ok(SingleRun {
        elapsed_seconds,
        messages,
        wire_bytes: wire_bytes
            .checked_mul(multiplier)
            .ok_or_else(|| BenchmarkError::Config("wire byte total overflows".to_owned()))?,
        decoded_bytes: decoded_bytes
            .checked_mul(multiplier)
            .ok_or_else(|| BenchmarkError::Config("decoded byte total overflows".to_owned()))?,
        checksum: black_box(checksum),
        peak_rss_bytes: peak_rss_bytes()?,
    })
}

fn decode_path(path: &Path, access: AccessMode) -> Result<(u64, u64), BenchmarkError> {
    match access {
        AccessMode::Streaming => {
            let mut decoder = DynDecoder::from_file(path, VersionUpgradePolicy::UpgradeToV3)
                .map_err(|source| BenchmarkError::Dbn {
                    path: path.to_path_buf(),
                    source,
                })?;
            decode_all(&mut decoder, path)
        }
        AccessMode::FullyBufferedInput => {
            let bytes = fs::read(path).map_err(|source| BenchmarkError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let cursor = Cursor::new(bytes);
            let mut decoder =
                DynDecoder::inferred_with_buffer(cursor, VersionUpgradePolicy::UpgradeToV3)
                    .map_err(|source| BenchmarkError::Dbn {
                        path: path.to_path_buf(),
                        source,
                    })?;
            decode_all(&mut decoder, path)
        }
    }
}

fn decode_all(
    decoder: &mut impl DecodeRecordRef,
    path: &Path,
) -> Result<(u64, u64), BenchmarkError> {
    let mut count = 0_u64;
    let mut checksum = 0_u64;
    while let Some(record) = decoder
        .decode_record_ref()
        .map_err(|source| BenchmarkError::Dbn {
            path: path.to_path_buf(),
            source,
        })?
    {
        count += 1;
        checksum = checksum.wrapping_add(record.raw_index_ts());
        black_box(record.header().rtype);
    }
    Ok((count, checksum))
}

fn peak_rss_bytes() -> Result<Option<u64>, BenchmarkError> {
    let path = Path::new("/proc/self/status");
    if !path.is_file() {
        return Ok(None);
    }
    let mut text = String::new();
    File::open(path)
        .and_then(|mut file| file.read_to_string(&mut text))
        .map_err(|source| BenchmarkError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let value = text.lines().find_map(|line| {
        line.strip_prefix("VmHWM:").and_then(|suffix| {
            suffix
                .split_whitespace()
                .next()
                .and_then(|value| u64::from_str(value).ok())
        })
    });
    match value {
        Some(kilobytes) => kilobytes
            .checked_mul(1024)
            .map(Some)
            .ok_or_else(|| BenchmarkError::Config("VmHWM byte conversion overflowed".to_owned())),
        None => Ok(None),
    }
}

#[allow(clippy::cast_precision_loss)]
fn summarize_configuration(
    input: &InputVariant,
    access: AccessMode,
    concurrency: &'static str,
    threads: usize,
    warmups: usize,
    samples: &[SingleRun],
) -> Result<BenchmarkResult, BenchmarkError> {
    let first = samples
        .first()
        .ok_or_else(|| BenchmarkError::Config("configuration has no samples".to_owned()))?;
    if samples.iter().any(|sample| {
        sample.messages != first.messages
            || sample.wire_bytes != first.wire_bytes
            || sample.decoded_bytes != first.decoded_bytes
    }) {
        return Err(BenchmarkError::Config(
            "configuration sample work totals differ".to_owned(),
        ));
    }
    let elapsed: Vec<f64> = samples
        .iter()
        .map(|sample| sample.elapsed_seconds)
        .collect();
    let messages_per_second: Vec<f64> = samples
        .iter()
        .map(|sample| sample.messages as f64 / sample.elapsed_seconds)
        .collect();
    let elapsed_stats = statistics(&elapsed)?;
    let message_stats = statistics(&messages_per_second)?;
    let peak_rss: Vec<f64> = samples
        .iter()
        .filter_map(|sample| sample.peak_rss_bytes.map(|bytes| bytes as f64 / MIB))
        .collect();
    let peak_stats = (!peak_rss.is_empty())
        .then(|| statistics(&peak_rss))
        .transpose()?;
    Ok(BenchmarkResult {
        schema: input.schema.clone(),
        compression: input.compression,
        access,
        concurrency,
        threads,
        measured_runs: samples.len(),
        warmup_runs_discarded: warmups,
        messages_per_run: first.messages,
        wire_bytes_per_run: first.wire_bytes,
        decoded_bytes_per_run: first.decoded_bytes,
        elapsed_seconds_median: elapsed_stats.median,
        elapsed_seconds_p95: elapsed_stats.p95,
        elapsed_seconds_stddev: elapsed_stats.stddev,
        messages_per_second_median: message_stats.median,
        messages_per_second_p95: message_stats.p95,
        messages_per_second_stddev: message_stats.stddev,
        wire_mib_per_second_median: first.wire_bytes as f64 / MIB / elapsed_stats.median,
        decoded_mib_per_second_median: first.decoded_bytes as f64 / MIB / elapsed_stats.median,
        nanoseconds_per_message_median: elapsed_stats.median * 1_000_000_000.0
            / first.messages as f64,
        peak_rss_mib_median: peak_stats.map(|stats| stats.median),
        peak_rss_mib_max: peak_rss.into_iter().reduce(f64::max),
        allocation_count: None,
    })
}

struct Statistics {
    median: f64,
    p95: f64,
    stddev: f64,
}

#[allow(clippy::cast_precision_loss)]
fn statistics(values: &[f64]) -> Result<Statistics, BenchmarkError> {
    if values.is_empty()
        || values
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return Err(BenchmarkError::Config(
            "statistics require positive finite samples".to_owned(),
        ));
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let middle = sorted.len() / 2;
    let median = if sorted.len() & 1 == 0 {
        sorted[middle - 1].mul_add(0.5, sorted[middle] * 0.5)
    } else {
        sorted[middle]
    };
    let p95_index = (sorted.len() * 95).div_ceil(100).saturating_sub(1);
    let p95 = sorted[p95_index];
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let variance = sorted
        .iter()
        .map(|value| {
            let delta = value - mean;
            delta * delta
        })
        .sum::<f64>()
        / sorted.len() as f64;
    Ok(Statistics {
        median,
        p95,
        stddev: variance.sqrt(),
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, BenchmarkError> {
    let file = File::open(path).map_err(|source| BenchmarkError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_reader(BufReader::new(file)).map_err(|source| BenchmarkError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), BenchmarkError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| BenchmarkError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let part = part_path(path);
    let file = File::create(&part).map_err(|source| BenchmarkError::Io {
        path: part.clone(),
        source,
    })?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value).map_err(|source| BenchmarkError::Json {
        path: part.clone(),
        source,
    })?;
    writer
        .write_all(b"\n")
        .map_err(|source| BenchmarkError::Io {
            path: part.clone(),
            source,
        })?;
    writer.flush().map_err(|source| BenchmarkError::Io {
        path: part.clone(),
        source,
    })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| BenchmarkError::Io {
            path: part.clone(),
            source,
        })?;
    if path.exists() {
        fs::remove_file(path).map_err(|source| BenchmarkError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    fs::rename(&part, path).map_err(|source| BenchmarkError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn part_path(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "{}.part",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("tmp")
    ))
}

#[cfg(test)]
mod tests {
    use super::statistics;

    #[test]
    fn aggregates_five_runs_deterministically() {
        let stats = statistics(&[1.0, 5.0, 3.0, 2.0, 4.0]).expect("valid samples");
        assert!((stats.median - 3.0).abs() < f64::EPSILON);
        assert!((stats.p95 - 5.0).abs() < f64::EPSILON);
        assert!((stats.stddev - 2.0_f64.sqrt()).abs() < f64::EPSILON);
    }
}
