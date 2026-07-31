use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use dbn_es_core::{DecodeStats, decode_stats};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, thiserror::Error)]
pub enum DecodeCommandError {
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
    #[error(transparent)]
    Decode(#[from] dbn_es_core::DecodeError),
    #[error("manifest verification failed for {path}: {reason}")]
    Manifest { path: PathBuf, reason: String },
    #[error("failed to format report timestamp: {0}")]
    Timestamp(#[from] time::error::Format),
}

#[derive(Debug, Deserialize)]
struct Manifest {
    entries: Vec<ManifestEntry>,
}

#[derive(Debug, Deserialize)]
struct ManifestEntry {
    path: PathBuf,
    schema: String,
    record_count: u64,
}

#[derive(Debug, Serialize)]
pub struct DecodeReport {
    version: u32,
    generated_at: String,
    manifest_path: PathBuf,
    files: Vec<DecodeStats>,
    passed: bool,
}

pub fn run_stats(
    manifest_path: &Path,
    output_path: &Path,
) -> Result<DecodeReport, DecodeCommandError> {
    let manifest = read_manifest(manifest_path)?;
    if manifest.entries.is_empty() {
        return Err(DecodeCommandError::Manifest {
            path: manifest_path.to_path_buf(),
            reason: "manifest has no entries".to_owned(),
        });
    }

    let mut files = Vec::with_capacity(manifest.entries.len());
    for entry in manifest.entries {
        let stats = decode_stats(&entry.path)?;
        if stats.schema.as_deref() != Some(entry.schema.as_str()) {
            return Err(DecodeCommandError::Manifest {
                path: entry.path,
                reason: format!(
                    "schema mismatch: manifest={}, decoded={}",
                    entry.schema,
                    stats.schema.as_deref().unwrap_or("mixed/unspecified")
                ),
            });
        }
        if stats.record_count != entry.record_count {
            return Err(DecodeCommandError::Manifest {
                path: entry.path,
                reason: format!(
                    "record count mismatch: manifest={}, decoded={}",
                    entry.record_count, stats.record_count
                ),
            });
        }
        files.push(stats);
    }

    let report = DecodeReport {
        version: 1,
        generated_at: OffsetDateTime::now_utc().format(&Rfc3339)?,
        manifest_path: manifest_path.to_path_buf(),
        files,
        passed: true,
    };
    write_json_atomic(output_path, &report)?;
    Ok(report)
}

fn read_manifest(path: &Path) -> Result<Manifest, DecodeCommandError> {
    let file = File::open(path).map_err(|source| DecodeCommandError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_reader(BufReader::new(file)).map_err(|source| DecodeCommandError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), DecodeCommandError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| DecodeCommandError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let part_path = path.with_extension(format!(
        "{}.part",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("tmp")
    ));
    let file = File::create(&part_path).map_err(|source| DecodeCommandError::Io {
        path: part_path.clone(),
        source,
    })?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value).map_err(|source| {
        DecodeCommandError::Json {
            path: part_path.clone(),
            source,
        }
    })?;
    writer
        .write_all(b"\n")
        .map_err(|source| DecodeCommandError::Io {
            path: part_path.clone(),
            source,
        })?;
    writer.flush().map_err(|source| DecodeCommandError::Io {
        path: part_path.clone(),
        source,
    })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| DecodeCommandError::Io {
            path: part_path.clone(),
            source,
        })?;
    if path.exists() {
        fs::remove_file(path).map_err(|source| DecodeCommandError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    fs::rename(&part_path, path).map_err(|source| DecodeCommandError::Io {
        path: path.to_path_buf(),
        source,
    })
}
