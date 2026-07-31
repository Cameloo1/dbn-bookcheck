use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as FmtWrite,
    fs::{self, File},
    io::{self, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use databento::{
    HistoricalClient,
    dbn::{
        SType, Schema,
        decode::{DbnDecoder, DbnMetadata, DecodeRecordRef},
    },
    historical::{DateTimeRange, metadata::GetCostParams, timeseries::GetRangeParams},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum AcquisitionError {
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
    #[error("invalid RFC 3339 timestamp {value:?}: {source}")]
    Timestamp {
        value: String,
        #[source]
        source: time::error::Parse,
    },
    #[error("unsupported DBN schema {0:?}")]
    Schema(String),
    #[error("Databento request failed: {0}")]
    Databento(#[from] databento::Error),
    #[error("DBN operation failed: {0}")]
    Dbn(#[from] dbn::Error),
    #[error("JSON serialization failed: {0}")]
    JsonValue(#[from] serde_json::Error),
    #[error("zstd decoding failed: {0}")]
    Zstd(#[from] io::Error),
    #[error("quoted request total ${quoted:.6} exceeds configured cap ${cap:.2}")]
    SpendCap { quoted: f64, cap: f64 },
    #[error(
        "request {request_id} has already been issued with status {status}; refusing a potentially billable replay"
    )]
    ReplayGuard { request_id: String, status: String },
    #[error("manifest verification failed: {0}")]
    Verification(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct AcquisitionConfig {
    pub dataset: String,
    pub symbol: String,
    pub stype_in: String,
    pub start: String,
    pub end: String,
    pub schemas: Vec<String>,
    pub spend_cap_usd: f64,
    pub output_dir: PathBuf,
    pub session_reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuoteLine {
    pub schema: String,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuoteReport {
    pub quoted_at: String,
    pub dataset: String,
    pub symbol: String,
    pub stype_in: String,
    pub start: String,
    pub end: String,
    pub lines: Vec<QuoteLine>,
    pub total_cost_usd: f64,
    pub spend_cap_usd: f64,
    pub within_cap: bool,
    pub download_issued: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ManifestEntry {
    pub path: String,
    pub schema: String,
    pub source: String,
    pub dataset: String,
    pub symbol: String,
    pub start: String,
    pub end: String,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub record_count: u64,
    pub sha256: String,
    pub acquired_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub version: u32,
    pub generated_at: String,
    pub source: String,
    pub session_reason: String,
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SpendRequest {
    request_id: String,
    schema: String,
    quoted_cost_usd: f64,
    status: String,
    issued_at: String,
    completed_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SpendState {
    version: u32,
    cap_usd: f64,
    conservatively_counted_usd: f64,
    requests: Vec<SpendRequest>,
}

#[derive(Debug, Serialize)]
pub struct VerificationReport {
    pub verified_at: String,
    pub manifest_path: String,
    pub source: String,
    pub files_verified: usize,
    pub schemas_verified: Vec<String>,
    pub conservatively_counted_usd: f64,
    pub spend_cap_usd: f64,
    pub passed: bool,
}

pub fn load_config(path: &Path) -> Result<AcquisitionConfig, AcquisitionError> {
    let file = File::open(path).map_err(|source| AcquisitionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_reader(BufReader::new(file)).map_err(|source| AcquisitionError::Json {
        path: path.to_path_buf(),
        source,
    })
}

pub async fn quote(config: &AcquisitionConfig) -> Result<QuoteReport, AcquisitionError> {
    validate_config(config)?;
    let mut client = HistoricalClient::builder().key_from_env()?.build()?;
    let range = date_time_range(config)?;
    let mut lines = Vec::with_capacity(config.schemas.len());
    for schema_name in &config.schemas {
        let schema = parse_schema(schema_name)?;
        let params = GetCostParams::builder()
            .dataset(&config.dataset)
            .symbols(config.symbol.as_str())
            .schema(schema)
            .date_time_range(range.clone())
            .stype_in(SType::Continuous)
            .build();
        let cost_usd = client.metadata().get_cost(&params).await?;
        lines.push(QuoteLine {
            schema: schema_name.clone(),
            cost_usd,
        });
    }
    let total_cost_usd = lines.iter().map(|line| line.cost_usd).sum();
    let report = QuoteReport {
        quoted_at: now_rfc3339()?,
        dataset: config.dataset.clone(),
        symbol: config.symbol.clone(),
        stype_in: config.stype_in.clone(),
        start: config.start.clone(),
        end: config.end.clone(),
        lines,
        total_cost_usd,
        spend_cap_usd: config.spend_cap_usd,
        within_cap: total_cost_usd <= config.spend_cap_usd,
        download_issued: false,
    };
    fs::create_dir_all(&config.output_dir).map_err(|source| AcquisitionError::Io {
        path: config.output_dir.clone(),
        source,
    })?;
    write_json_atomic(&config.output_dir.join("quote.json"), &report)?;
    let spend_path = config.output_dir.join("spend-state.json");
    let spend = spend_path
        .is_file()
        .then(|| read_json(&spend_path))
        .transpose()?;
    write_spend_log(config, &report, spend.as_ref())?;
    Ok(report)
}

pub async fn acquire(config: &AcquisitionConfig) -> Result<Manifest, AcquisitionError> {
    let report = quote(config).await?;
    if !report.within_cap {
        return Err(AcquisitionError::SpendCap {
            quoted: report.total_cost_usd,
            cap: report.spend_cap_usd,
        });
    }

    let spend_path = config.output_dir.join("spend-state.json");
    let mut spend = load_spend_state(&spend_path, config.spend_cap_usd)?;
    let mut entries_by_schema = load_existing_manifest(config)?;
    let mut client = HistoricalClient::builder().key_from_env()?.build()?;
    let range = date_time_range(config)?;

    for line in &report.lines {
        let schema = parse_schema(&line.schema)?;
        let file_name = data_file_name(config, &line.schema);
        let final_path = config.output_dir.join(&file_name);
        if reuse_existing_file(config, line, &final_path, &spend, &mut entries_by_schema)? {
            continue;
        }

        let request_id = request_id(config, &line.schema);
        if let Some(previous) = spend
            .requests
            .iter()
            .find(|request| request.request_id == request_id)
        {
            return Err(AcquisitionError::ReplayGuard {
                request_id,
                status: previous.status.clone(),
            });
        }
        let projected = spend.conservatively_counted_usd + line.cost_usd;
        if projected > spend.cap_usd {
            return Err(AcquisitionError::SpendCap {
                quoted: projected,
                cap: spend.cap_usd,
            });
        }

        let issued_at = now_rfc3339()?;
        spend.conservatively_counted_usd = projected;
        spend.requests.push(SpendRequest {
            request_id: request_id.clone(),
            schema: line.schema.clone(),
            quoted_cost_usd: line.cost_usd,
            status: "issued".to_owned(),
            issued_at,
            completed_at: None,
        });
        write_json_atomic(&spend_path, &spend)?;
        write_spend_log(config, &report, Some(&spend))?;

        let part_path = config.output_dir.join(format!("{file_name}.part"));
        let params = GetRangeParams::builder()
            .dataset(&config.dataset)
            .symbols(config.symbol.as_str())
            .schema(schema)
            .date_time_range(range.clone())
            .stype_in(SType::Continuous)
            .build()
            .with_path(&part_path);
        match client.timeseries().get_range_to_file(&params).await {
            Ok(decoder) => {
                drop(decoder);
                fs::rename(&part_path, &final_path).map_err(|source| AcquisitionError::Io {
                    path: final_path.clone(),
                    source,
                })?;
                let completed_at = now_rfc3339()?;
                if let Some(request) = spend
                    .requests
                    .iter_mut()
                    .find(|request| request.request_id == request_id)
                {
                    "completed".clone_into(&mut request.status);
                    request.completed_at = Some(completed_at.clone());
                }
                write_json_atomic(&spend_path, &spend)?;
                write_spend_log(config, &report, Some(&spend))?;
                let entry = inspect_file(config, &line.schema, &final_path, completed_at)?;
                entries_by_schema.insert(line.schema.clone(), entry);
                write_manifest(config, entries_by_schema.values().cloned().collect())?;
            }
            Err(error) => {
                if let Some(request) = spend
                    .requests
                    .iter_mut()
                    .find(|request| request.request_id == request_id)
                {
                    "failed_or_delivery_uncertain".clone_into(&mut request.status);
                }
                write_json_atomic(&spend_path, &spend)?;
                write_spend_log(config, &report, Some(&spend))?;
                return Err(error.into());
            }
        }
    }

    finalize_acquisition(config, &report, &spend, entries_by_schema)
}

fn reuse_existing_file(
    config: &AcquisitionConfig,
    line: &QuoteLine,
    path: &Path,
    spend: &SpendState,
    entries_by_schema: &mut BTreeMap<String, ManifestEntry>,
) -> Result<bool, AcquisitionError> {
    if !path.is_file() {
        return Ok(false);
    }
    let acquired_at = recorded_acquired_at(
        config,
        &line.schema,
        spend,
        entries_by_schema.get(&line.schema),
    )?;
    let existing = inspect_file(config, &line.schema, path, acquired_at)?;
    entries_by_schema.insert(line.schema.clone(), existing);
    Ok(true)
}

fn finalize_acquisition(
    config: &AcquisitionConfig,
    report: &QuoteReport,
    spend: &SpendState,
    entries_by_schema: BTreeMap<String, ManifestEntry>,
) -> Result<Manifest, AcquisitionError> {
    write_spend_log(config, report, Some(spend))?;
    let manifest = write_manifest(config, entries_by_schema.into_values().collect())?;
    verify(config)?;
    Ok(manifest)
}

pub fn verify(config: &AcquisitionConfig) -> Result<VerificationReport, AcquisitionError> {
    validate_config(config)?;
    let manifest_path = config.output_dir.join("manifest.json");
    let manifest: Manifest = read_json(&manifest_path)?;
    if manifest.source != "live" {
        return Err(AcquisitionError::Verification(format!(
            "expected live manifest source, found {}",
            manifest.source
        )));
    }
    let expected_schemas: BTreeSet<_> = config.schemas.iter().cloned().collect();
    let actual_schemas: BTreeSet<_> = manifest
        .entries
        .iter()
        .map(|entry| entry.schema.clone())
        .collect();
    if expected_schemas != actual_schemas {
        return Err(AcquisitionError::Verification(format!(
            "schema set mismatch: expected {expected_schemas:?}, found {actual_schemas:?}"
        )));
    }
    for entry in &manifest.entries {
        let path = PathBuf::from(&entry.path);
        let actual = inspect_file(config, &entry.schema, &path, entry.acquired_at.clone())?;
        if actual.compressed_bytes != entry.compressed_bytes
            || actual.uncompressed_bytes != entry.uncompressed_bytes
            || actual.record_count != entry.record_count
            || actual.sha256 != entry.sha256
        {
            return Err(AcquisitionError::Verification(format!(
                "manifest mismatch for {}",
                entry.path
            )));
        }
    }
    let spend: SpendState = read_json(&config.output_dir.join("spend-state.json"))?;
    if spend.conservatively_counted_usd > spend.cap_usd
        || (spend.cap_usd - config.spend_cap_usd).abs() > f64::EPSILON
    {
        return Err(AcquisitionError::Verification(format!(
            "spend state ${:.6} does not reconcile to cap ${:.2}",
            spend.conservatively_counted_usd, config.spend_cap_usd
        )));
    }
    for entry in &manifest.entries {
        let expected_request_id = request_id(config, &entry.schema);
        let request = spend
            .requests
            .iter()
            .find(|request| request.request_id == expected_request_id)
            .ok_or_else(|| {
                AcquisitionError::Verification(format!(
                    "no spend-ledger request found for schema {}",
                    entry.schema
                ))
            })?;
        if request.status != "completed"
            || request.completed_at.as_deref() != Some(entry.acquired_at.as_str())
        {
            return Err(AcquisitionError::Verification(format!(
                "acquisition timestamp or request status mismatch for schema {}",
                entry.schema
            )));
        }
    }
    let quote: QuoteReport = read_json(&config.output_dir.join("quote.json"))?;
    let expected_log = render_spend_log(&quote, Some(&spend))?;
    let spend_log_path = config.output_dir.join("spend-log.md");
    let actual_log =
        fs::read_to_string(&spend_log_path).map_err(|source| AcquisitionError::Io {
            path: spend_log_path,
            source,
        })?;
    if actual_log != expected_log {
        return Err(AcquisitionError::Verification(
            "spend-log.md is not the deterministic rendering of quote.json and spend-state.json"
                .to_owned(),
        ));
    }
    Ok(VerificationReport {
        verified_at: now_rfc3339()?,
        manifest_path: manifest_path.to_string_lossy().replace('\\', "/"),
        source: manifest.source,
        files_verified: manifest.entries.len(),
        schemas_verified: actual_schemas.into_iter().collect(),
        conservatively_counted_usd: spend.conservatively_counted_usd,
        spend_cap_usd: spend.cap_usd,
        passed: true,
    })
}

fn validate_config(config: &AcquisitionConfig) -> Result<(), AcquisitionError> {
    if config.stype_in != "continuous" {
        return Err(AcquisitionError::Verification(format!(
            "only continuous input symbology is supported, found {}",
            config.stype_in
        )));
    }
    if config.schemas.is_empty() {
        return Err(AcquisitionError::Verification(
            "at least one schema is required".to_owned(),
        ));
    }
    if !config.spend_cap_usd.is_finite() || config.spend_cap_usd <= 0.0 {
        return Err(AcquisitionError::Verification(
            "spend cap must be finite and positive".to_owned(),
        ));
    }
    if config.spend_cap_usd > 10.0 {
        return Err(AcquisitionError::Verification(
            "spend cap cannot exceed the mission-wide $10.00 limit".to_owned(),
        ));
    }
    if config.output_dir.is_absolute()
        || config
            .output_dir
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
    {
        return Err(AcquisitionError::Verification(
            "output_dir must be a repository-relative path without parent traversal".to_owned(),
        ));
    }
    let range = date_time_range(config)?;
    if range.start >= range.end {
        return Err(AcquisitionError::Verification(
            "start must precede end".to_owned(),
        ));
    }
    if range.end - range.start > time::Duration::days(1) {
        return Err(AcquisitionError::Verification(
            "acquisition range cannot exceed 24 hours".to_owned(),
        ));
    }
    let unique_schemas: BTreeSet<_> = config.schemas.iter().collect();
    if unique_schemas.len() != config.schemas.len() {
        return Err(AcquisitionError::Verification(
            "schema list contains duplicates".to_owned(),
        ));
    }
    for schema in &config.schemas {
        parse_schema(schema)?;
    }
    Ok(())
}

fn parse_schema(value: &str) -> Result<Schema, AcquisitionError> {
    Schema::from_str(value).map_err(|_| AcquisitionError::Schema(value.to_owned()))
}

fn date_time_range(config: &AcquisitionConfig) -> Result<DateTimeRange, AcquisitionError> {
    let start = OffsetDateTime::parse(&config.start, &Rfc3339).map_err(|source| {
        AcquisitionError::Timestamp {
            value: config.start.clone(),
            source,
        }
    })?;
    let end = OffsetDateTime::parse(&config.end, &Rfc3339).map_err(|source| {
        AcquisitionError::Timestamp {
            value: config.end.clone(),
            source,
        }
    })?;
    Ok((start, end).into())
}

fn inspect_file(
    config: &AcquisitionConfig,
    schema_name: &str,
    path: &Path,
    acquired_at: String,
) -> Result<ManifestEntry, AcquisitionError> {
    let compressed_bytes = fs::metadata(path)
        .map_err(|source| AcquisitionError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let mut decoder = DbnDecoder::from_zstd_file(path)?;
    if decoder.metadata().schema != Some(parse_schema(schema_name)?) {
        return Err(AcquisitionError::Verification(format!(
            "DBN metadata schema mismatch in {}",
            path.display()
        )));
    }
    let mut record_count = 0_u64;
    while decoder.decode_record_ref()?.is_some() {
        record_count = record_count.saturating_add(1);
    }

    let file = File::open(path).map_err(|source| AcquisitionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut zstd = zstd::stream::read::Decoder::new(file)?;
    let uncompressed_bytes = io::copy(&mut zstd, &mut io::sink())?;
    let sha256 = sha256(path)?;
    Ok(ManifestEntry {
        path: path.to_string_lossy().replace('\\', "/"),
        schema: schema_name.to_owned(),
        source: "live".to_owned(),
        dataset: config.dataset.clone(),
        symbol: config.symbol.clone(),
        start: config.start.clone(),
        end: config.end.clone(),
        compressed_bytes,
        uncompressed_bytes,
        record_count,
        sha256,
        acquired_at,
    })
}

fn sha256(path: &Path) -> Result<String, AcquisitionError> {
    let mut file = File::open(path).map_err(|source| AcquisitionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| AcquisitionError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn write_manifest(
    config: &AcquisitionConfig,
    mut entries: Vec<ManifestEntry>,
) -> Result<Manifest, AcquisitionError> {
    entries.sort_by(|left, right| left.schema.cmp(&right.schema));
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        generated_at: now_rfc3339()?,
        source: "live".to_owned(),
        session_reason: config.session_reason.clone(),
        entries,
    };
    write_json_atomic(&config.output_dir.join("manifest.json"), &manifest)?;
    Ok(manifest)
}

fn load_existing_manifest(
    config: &AcquisitionConfig,
) -> Result<BTreeMap<String, ManifestEntry>, AcquisitionError> {
    let path = config.output_dir.join("manifest.json");
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let manifest: Manifest = read_json(&path)?;
    Ok(manifest
        .entries
        .into_iter()
        .map(|entry| (entry.schema.clone(), entry))
        .collect())
}

fn load_spend_state(path: &Path, cap_usd: f64) -> Result<SpendState, AcquisitionError> {
    if path.is_file() {
        let state: SpendState = read_json(path)?;
        if (state.cap_usd - cap_usd).abs() > f64::EPSILON
            || state.conservatively_counted_usd > state.cap_usd
        {
            return Err(AcquisitionError::Verification(
                "existing spend state does not reconcile to the configured cap".to_owned(),
            ));
        }
        return Ok(state);
    }
    Ok(SpendState {
        version: 1,
        cap_usd,
        conservatively_counted_usd: 0.0,
        requests: Vec::new(),
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, AcquisitionError> {
    let file = File::open(path).map_err(|source| AcquisitionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_reader(BufReader::new(file)).map_err(|source| AcquisitionError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), AcquisitionError> {
    let temp_path = path.with_extension("tmp");
    let mut file = File::create(&temp_path).map_err(|source| AcquisitionError::Io {
        path: temp_path.clone(),
        source,
    })?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")
        .map_err(|source| AcquisitionError::Io {
            path: temp_path.clone(),
            source,
        })?;
    file.sync_all().map_err(|source| AcquisitionError::Io {
        path: temp_path.clone(),
        source,
    })?;
    replace_file(&temp_path, path)
}

fn replace_file(temp_path: &Path, path: &Path) -> Result<(), AcquisitionError> {
    let backup_path = path.with_extension("bak");
    if backup_path.is_file() {
        fs::remove_file(&backup_path).map_err(|source| AcquisitionError::Io {
            path: backup_path.clone(),
            source,
        })?;
    }
    if path.is_file() {
        fs::rename(path, &backup_path).map_err(|source| AcquisitionError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    if let Err(source) = fs::rename(temp_path, path) {
        if backup_path.is_file() {
            let _restore_result = fs::rename(&backup_path, path);
        }
        return Err(AcquisitionError::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    if backup_path.is_file() {
        fs::remove_file(&backup_path).map_err(|source| AcquisitionError::Io {
            path: backup_path,
            source,
        })?;
    }
    Ok(())
}

fn write_spend_log(
    config: &AcquisitionConfig,
    report: &QuoteReport,
    spend: Option<&SpendState>,
) -> Result<(), AcquisitionError> {
    let path = config.output_dir.join("spend-log.md");
    let temp_path = path.with_extension("md.tmp");
    let content = render_spend_log(report, spend)?;
    let mut file = File::create(&temp_path).map_err(|source| AcquisitionError::Io {
        path: temp_path.clone(),
        source,
    })?;
    file.write_all(content.as_bytes())
        .map_err(|source| AcquisitionError::Io {
            path: temp_path.clone(),
            source,
        })?;
    file.sync_all().map_err(|source| AcquisitionError::Io {
        path: temp_path.clone(),
        source,
    })?;
    replace_file(&temp_path, &path)
}

fn render_spend_log(
    report: &QuoteReport,
    spend: Option<&SpendState>,
) -> Result<String, AcquisitionError> {
    let mut output = String::new();
    writeln!(
        output,
        "# Data spend log\n\nGenerated from `quote.json` and `spend-state.json`; do not edit by hand.\n\n## Latest quote — {}\n\n- Dataset: `{}`\n- Symbol: `{}` (`{}`)\n- Range: `{}` to `{}`\n- Configured cap: `${:.2}`\n\n| Schema | Quoted cost USD |\n| --- | ---: |",
        report.quoted_at,
        report.dataset,
        report.symbol,
        report.stype_in,
        report.start,
        report.end,
        report.spend_cap_usd
    )
    .map_err(|error| {
        AcquisitionError::Verification(format!("spend log formatting failed: {error}"))
    })?;
    for line in &report.lines {
        writeln!(output, "| {} | ${:.6} |", line.schema, line.cost_usd).map_err(|error| {
            AcquisitionError::Verification(format!("spend log formatting failed: {error}"))
        })?;
    }
    writeln!(
        output,
        "\nTotal quoted: `${:.6}`. Within cap: `{}`.\n",
        report.total_cost_usd, report.within_cap
    )
    .map_err(|error| {
        AcquisitionError::Verification(format!("spend log formatting failed: {error}"))
    })?;
    if let Some(spend) = spend {
        writeln!(
            output,
            "## Request ledger\n\nConservatively counted: `${:.6}` of `${:.2}`. An issued, failed, or delivery-uncertain request remains counted.\n\n| Schema | Request ID | Quoted cost USD | Status | Issued at | Completed at |\n| --- | --- | ---: | --- | --- | --- |",
            spend.conservatively_counted_usd, spend.cap_usd
        )
        .map_err(|error| {
            AcquisitionError::Verification(format!("spend log formatting failed: {error}"))
        })?;
        for request in &spend.requests {
            writeln!(
                output,
                "| {} | `{}` | ${:.6} | {} | {} | {} |",
                request.schema,
                request.request_id,
                request.quoted_cost_usd,
                request.status,
                request.issued_at,
                request.completed_at.as_deref().unwrap_or("—")
            )
            .map_err(|error| {
                AcquisitionError::Verification(format!("spend log formatting failed: {error}"))
            })?;
        }
        output.push('\n');
    }
    Ok(output)
}

fn data_file_name(config: &AcquisitionConfig, schema: &str) -> String {
    let symbol = config.symbol.replace('.', "-").to_ascii_lowercase();
    let start = file_timestamp(&config.start);
    let end = file_timestamp(&config.end);
    format!("{symbol}_{start}_{end}_{schema}.dbn.zst")
}

fn file_timestamp(value: &str) -> String {
    value
        .replace([':', '-'], "")
        .replace("+00:00", "Z")
        .to_ascii_lowercase()
}

fn request_id(config: &AcquisitionConfig, schema: &str) -> String {
    let mut hasher = Sha256::new();
    for value in [
        config.dataset.as_str(),
        config.symbol.as_str(),
        config.stype_in.as_str(),
        config.start.as_str(),
        config.end.as_str(),
        schema,
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

fn recorded_acquired_at(
    config: &AcquisitionConfig,
    schema: &str,
    spend: &SpendState,
    manifest_entry: Option<&ManifestEntry>,
) -> Result<String, AcquisitionError> {
    let expected_request_id = request_id(config, schema);
    if let Some(completed_at) = spend
        .requests
        .iter()
        .find(|request| request.request_id == expected_request_id)
        .and_then(|request| request.completed_at.clone())
    {
        return Ok(completed_at);
    }
    manifest_entry
        .map(|entry| entry.acquired_at.clone())
        .ok_or_else(|| {
            AcquisitionError::Verification(format!(
                "existing file for schema {schema} has no recorded acquisition timestamp"
            ))
        })
}

fn now_rfc3339() -> Result<String, AcquisitionError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|error| AcquisitionError::Verification(format!("time formatting failed: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AcquisitionConfig {
        AcquisitionConfig {
            dataset: "GLBX.MDP3".to_owned(),
            symbol: "ES.v.0".to_owned(),
            stype_in: "continuous".to_owned(),
            start: "2025-04-03T22:00:00Z".to_owned(),
            end: "2025-04-04T21:00:00Z".to_owned(),
            schemas: vec![
                "mbo".to_owned(),
                "mbp-10".to_owned(),
                "trades".to_owned(),
                "ohlcv-1m".to_owned(),
            ],
            spend_cap_usd: 10.0,
            output_dir: PathBuf::from("data"),
            session_reason: "test session".to_owned(),
        }
    }

    #[test]
    fn committed_config_is_bounded() {
        let config = load_config(Path::new("../../config/live-session.json"))
            .expect("committed config should load");
        validate_config(&config).expect("committed config should be safe");
    }

    #[test]
    fn rejects_cap_above_mission_limit() {
        let mut config = config();
        config.spend_cap_usd = 10.01;
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn rejects_unbounded_or_traversing_output() {
        let mut config = config();
        config.output_dir = PathBuf::from("../outside");
        assert!(validate_config(&config).is_err());
        config.output_dir = PathBuf::from("data");
        config.end = "2025-04-05T22:00:01Z".to_owned();
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn request_ids_are_stable_and_schema_specific() {
        let config = config();
        assert_eq!(request_id(&config, "mbo"), request_id(&config, "mbo"));
        assert_ne!(request_id(&config, "mbo"), request_id(&config, "trades"));
    }

    #[test]
    fn completed_request_owns_acquisition_timestamp() {
        let config = config();
        let completed_at = "2026-07-30T22:31:32Z".to_owned();
        let spend = SpendState {
            version: 1,
            cap_usd: 10.0,
            conservatively_counted_usd: 1.0,
            requests: vec![SpendRequest {
                request_id: request_id(&config, "mbo"),
                schema: "mbo".to_owned(),
                quoted_cost_usd: 1.0,
                status: "completed".to_owned(),
                issued_at: "2026-07-30T22:30:00Z".to_owned(),
                completed_at: Some(completed_at.clone()),
            }],
        };
        assert_eq!(
            recorded_acquired_at(&config, "mbo", &spend, None)
                .expect("completed request should provide timestamp"),
            completed_at
        );
    }
}
