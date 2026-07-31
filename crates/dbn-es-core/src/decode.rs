use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{self, BufReader, Read},
    marker::PhantomData,
    path::{Path, PathBuf},
};

use dbn::{
    HasRType, MboMsg, Mbp10Msg, OhlcvMsg, Record, RecordRef, Schema, TradeMsg,
    VersionUpgradePolicy,
    decode::{
        DbnDecoder, DbnMetadata, DecodeRecordRef, DecodeStream, DynReader, StreamIterDecoder,
    },
};
use fallible_streaming_iterator::FallibleStreamingIterator;
use serde::Serialize;

type FileSource = DynReader<'static, BufReader<File>>;
type FileDecoder = DbnDecoder<StrictDbnReader<FileSource>>;

/// Pass-through reader that turns an EOF in the middle of DBN metadata or a record
/// into `InvalidData`. The upstream decoder intentionally maps `UnexpectedEof` to
/// clean exhaustion for record streams, so this boundary tracker prevents truncated
/// inputs from being accepted silently without buffering the file or adding a second
/// validation pass.
struct StrictDbnReader<R> {
    inner: R,
    prefix: Vec<u8>,
    position: u64,
    metadata_end: Option<u64>,
    record_remaining: usize,
}

impl<R> StrictDbnReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            prefix: Vec::with_capacity(8),
            position: 0,
            metadata_end: None,
            record_remaining: 0,
        }
    }

    fn observe(&mut self, bytes: &[u8]) {
        let mut offset = 0;
        if self.prefix.len() < 8 {
            let take = (8 - self.prefix.len()).min(bytes.len());
            self.prefix.extend_from_slice(&bytes[..take]);
            self.position += take as u64;
            offset += take;
            if self.prefix.len() == 8 {
                let metadata_length = u32::from_le_bytes([
                    self.prefix[4],
                    self.prefix[5],
                    self.prefix[6],
                    self.prefix[7],
                ]);
                self.metadata_end = Some(8 + u64::from(metadata_length));
            }
        }

        if let Some(metadata_end) = self.metadata_end
            && self.position < metadata_end
        {
            let available = bytes.len() - offset;
            let remaining = usize::try_from(metadata_end - self.position).unwrap_or(usize::MAX);
            let take = available.min(remaining);
            self.position += take as u64;
            offset += take;
        }

        while offset < bytes.len() {
            if self.record_remaining == 0 {
                self.record_remaining = usize::from(bytes[offset]) * 4;
            }
            let take = self.record_remaining.min(bytes.len() - offset);
            self.record_remaining -= take;
            self.position += take as u64;
            offset += take;
        }
    }

    fn is_complete(&self) -> bool {
        self.metadata_end
            .is_some_and(|metadata_end| self.position >= metadata_end)
            && self.record_remaining == 0
    }
}

impl<R: Read> Read for StrictDbnReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        match self.inner.read(buffer) {
            Ok(0) if self.is_complete() => Ok(0),
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated DBN metadata or record",
            )),
            Ok(read) => {
                self.observe(&buffer[..read]);
                Ok(read)
            }
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated compressed DBN stream",
            )),
            Err(error) => Err(error),
        }
    }
}

/// Errors returned by the DBN core decoder.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// The official DBN decoder rejected the file or a record.
    #[error("DBN decode failed for {path}: {source}")]
    Dbn {
        /// File being decoded.
        path: PathBuf,
        /// Original DBN error.
        #[source]
        source: dbn::Error,
    },
    /// A typed stream was requested for a file containing another schema.
    #[error("schema mismatch for {path}: expected {expected}, metadata reported {actual}")]
    SchemaMismatch {
        /// File being decoded.
        path: PathBuf,
        /// Schema required by the typed constructor.
        expected: &'static str,
        /// Schema found in DBN metadata.
        actual: String,
    },
}

/// A typed, fallible, zero-copy record stream.
///
/// [`TypedStream::next_record`] borrows the decoder-owned record buffer. Advancing
/// the stream invalidates that borrow, which is why this deliberately is not a
/// standard `Iterator`.
pub struct TypedStream<T: HasRType> {
    inner: StreamIterDecoder<FileDecoder, T>,
    path: PathBuf,
    _record: PhantomData<T>,
}

impl<T: HasRType> TypedStream<T> {
    /// Advances the stream and returns the next borrowed record.
    ///
    /// # Errors
    /// Returns [`DecodeError::Dbn`] for malformed, truncated, or otherwise invalid
    /// DBN input.
    pub fn next_record(&mut self) -> Result<Option<&T>, DecodeError> {
        self.inner.advance().map_err(|source| DecodeError::Dbn {
            path: self.path.clone(),
            source,
        })?;
        Ok(self.inner.get())
    }

    /// Returns the schema declared by the DBN metadata.
    #[must_use]
    pub fn schema(&self) -> Option<Schema> {
        self.inner.metadata().schema
    }
}

/// Opens an MBO stream. Compression is inferred from the file header.
///
/// # Errors
/// Returns an error if the file cannot be decoded or is not MBO data.
pub fn stream_mbo(path: impl AsRef<Path>) -> Result<TypedStream<MboMsg>, DecodeError> {
    open_typed(path, Schema::Mbo)
}

/// Opens an MBP-10 stream. Compression is inferred from the file header.
///
/// # Errors
/// Returns an error if the file cannot be decoded or is not MBP-10 data.
pub fn stream_mbp10(path: impl AsRef<Path>) -> Result<TypedStream<Mbp10Msg>, DecodeError> {
    open_typed(path, Schema::Mbp10)
}

/// Opens a trades stream. Compression is inferred from the file header.
///
/// # Errors
/// Returns an error if the file cannot be decoded or is not trades data.
pub fn stream_trades(path: impl AsRef<Path>) -> Result<TypedStream<TradeMsg>, DecodeError> {
    open_typed(path, Schema::Trades)
}

/// Opens a one-minute OHLCV stream. Compression is inferred from the file header.
///
/// # Errors
/// Returns an error if the file cannot be decoded or is not one-minute OHLCV data.
pub fn stream_ohlcv_1m(path: impl AsRef<Path>) -> Result<TypedStream<OhlcvMsg>, DecodeError> {
    open_typed(path, Schema::Ohlcv1M)
}

fn open_typed<T: HasRType>(
    path: impl AsRef<Path>,
    expected: Schema,
) -> Result<TypedStream<T>, DecodeError> {
    let path = path.as_ref().to_path_buf();
    let decoder = open_decoder(&path)?;
    let actual = decoder.metadata().schema;
    if actual != Some(expected) {
        return Err(DecodeError::SchemaMismatch {
            path,
            expected: expected.as_str(),
            actual: actual
                .map_or_else(|| "mixed/unspecified".to_owned(), |value| value.to_string()),
        });
    }
    Ok(TypedStream {
        inner: decoder.decode_stream(),
        path,
        _record: PhantomData,
    })
}

fn open_decoder(path: &Path) -> Result<FileDecoder, DecodeError> {
    let reader = DynReader::from_file(path).map_err(|source| DecodeError::Dbn {
        path: path.to_path_buf(),
        source,
    })?;
    DbnDecoder::with_upgrade_policy(
        StrictDbnReader::new(reader),
        VersionUpgradePolicy::UpgradeToV3,
    )
    .map_err(|source| DecodeError::Dbn {
        path: path.to_path_buf(),
        source,
    })
}

/// One sampled sequence discontinuity from a schema that carries venue sequence
/// numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SequenceDiscontinuity {
    /// Publisher whose sequence stream was observed.
    pub publisher_id: u16,
    /// MBO channel when present; absent for schemas without a channel field.
    pub channel_id: Option<u8>,
    /// Previously observed sequence.
    pub previous: u32,
    /// Sequence that followed it.
    pub current: u32,
    /// Event timestamp on the current record.
    pub timestamp_ns: u64,
    /// Classification: `forward_gap`, `duplicate`, or `regression`.
    pub kind: &'static str,
    /// Missing sequence count for a forward gap, otherwise zero.
    pub missing: u64,
}

/// Aggregate sequence diagnostics for records that carry venue sequence numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SequenceStats {
    /// Records containing a sequence number.
    pub records_observed: u64,
    /// Number of forward discontinuities.
    pub forward_gap_count: u64,
    /// Total sequence values skipped by forward discontinuities.
    pub missing_sequence_count: u64,
    /// Repeated sequence values.
    pub duplicate_count: u64,
    /// Backward sequence movements excluding natural `u32` wraparound.
    pub regression_count: u64,
    /// Up to the first 100 discontinuities for inspection.
    pub samples: Vec<SequenceDiscontinuity>,
    /// Whether additional discontinuities were omitted from `samples`.
    pub samples_truncated: bool,
    /// Interpretation warning for symbol-filtered data.
    pub interpretation: &'static str,
}

impl Default for SequenceStats {
    fn default() -> Self {
        Self {
            records_observed: 0,
            forward_gap_count: 0,
            missing_sequence_count: 0,
            duplicate_count: 0,
            regression_count: 0,
            samples: Vec::new(),
            samples_truncated: false,
            interpretation: "Venue sequence gaps in a symbol-filtered stream may represent messages for unrequested instruments; they are diagnostics, not proof of data loss.",
        }
    }
}

/// Streaming decode diagnostics for one DBN file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecodeStats {
    /// Input path as supplied by the caller.
    pub path: PathBuf,
    /// Schema declared in DBN metadata.
    pub schema: Option<String>,
    /// Total decoded record count.
    pub record_count: u64,
    /// Counts keyed by DBN record type.
    pub record_counts_by_type: BTreeMap<String, u64>,
    /// Timestamp of the first decoded record in stream order.
    pub first_timestamp_ns: Option<u64>,
    /// Timestamp of the final decoded record in stream order.
    pub last_timestamp_ns: Option<u64>,
    /// Minimum timestamp observed.
    pub min_timestamp_ns: Option<u64>,
    /// Maximum timestamp observed.
    pub max_timestamp_ns: Option<u64>,
    /// Number of records whose timestamp is before the preceding record.
    pub timestamp_regression_count: u64,
    /// Sequence diagnostics, or `None` for schemas without venue sequence numbers.
    pub sequence: Option<SequenceStats>,
}

/// Decodes a DBN or Zstd-compressed DBN file and computes streaming diagnostics.
///
/// # Errors
/// Returns [`DecodeError::Dbn`] if metadata or any record is malformed.
pub fn decode_stats(path: impl AsRef<Path>) -> Result<DecodeStats, DecodeError> {
    let path = path.as_ref().to_path_buf();
    let mut decoder = open_decoder(&path)?;
    let schema = decoder.metadata().schema;
    let mut stats = DecodeStats {
        path: path.clone(),
        schema: schema.map(|value| value.to_string()),
        record_count: 0,
        record_counts_by_type: BTreeMap::new(),
        first_timestamp_ns: None,
        last_timestamp_ns: None,
        min_timestamp_ns: None,
        max_timestamp_ns: None,
        timestamp_regression_count: 0,
        sequence: schema
            .is_some_and(|value| matches!(value, Schema::Mbo | Schema::Mbp10 | Schema::Trades))
            .then(SequenceStats::default),
    };
    let mut previous_timestamp = None;
    let mut previous_sequences = HashMap::new();

    loop {
        let record = decoder
            .decode_record_ref()
            .map_err(|source| DecodeError::Dbn {
                path: path.clone(),
                source,
            })?;
        let Some(record) = record else {
            break;
        };
        update_record_stats(
            &mut stats,
            record,
            &mut previous_timestamp,
            &mut previous_sequences,
        )?;
    }
    Ok(stats)
}

type SequenceKey = (u16, Option<u8>);

fn update_record_stats(
    stats: &mut DecodeStats,
    record: RecordRef<'_>,
    previous_timestamp: &mut Option<u64>,
    previous_sequences: &mut HashMap<SequenceKey, u32>,
) -> Result<(), DecodeError> {
    let timestamp = record.raw_index_ts();
    stats.record_count += 1;
    stats.first_timestamp_ns.get_or_insert(timestamp);
    if previous_timestamp.is_some_and(|previous| timestamp < previous) {
        stats.timestamp_regression_count += 1;
    }
    *previous_timestamp = Some(timestamp);
    stats.last_timestamp_ns = Some(timestamp);
    stats.min_timestamp_ns = Some(
        stats
            .min_timestamp_ns
            .map_or(timestamp, |value| value.min(timestamp)),
    );
    stats.max_timestamp_ns = Some(
        stats
            .max_timestamp_ns
            .map_or(timestamp, |value| value.max(timestamp)),
    );

    let rtype = record.rtype().map_err(|source| DecodeError::Dbn {
        path: stats.path.clone(),
        source,
    })?;
    *stats
        .record_counts_by_type
        .entry(rtype.as_str().to_owned())
        .or_default() += 1;

    if let Some(sequence_stats) = stats.sequence.as_mut() {
        let observation = sequence_observation(record).map_err(|source| DecodeError::Dbn {
            path: stats.path.clone(),
            source,
        })?;
        if let Some((key, sequence)) = observation {
            sequence_stats.records_observed += 1;
            if let Some(previous) = previous_sequences.insert(key, sequence) {
                record_discontinuity(sequence_stats, key, previous, sequence, timestamp);
            }
        }
    }
    Ok(())
}

fn sequence_observation(record: RecordRef<'_>) -> dbn::Result<Option<(SequenceKey, u32)>> {
    if record.has::<MboMsg>() {
        let message = record.try_get::<MboMsg>()?;
        Ok(Some((
            (message.hd.publisher_id, Some(message.channel_id)),
            message.sequence,
        )))
    } else if record.has::<Mbp10Msg>() {
        let message = record.try_get::<Mbp10Msg>()?;
        Ok(Some(((message.hd.publisher_id, None), message.sequence)))
    } else if record.has::<TradeMsg>() {
        let message = record.try_get::<TradeMsg>()?;
        Ok(Some(((message.hd.publisher_id, None), message.sequence)))
    } else {
        Ok(None)
    }
}

fn record_discontinuity(
    stats: &mut SequenceStats,
    key: SequenceKey,
    previous: u32,
    current: u32,
    timestamp_ns: u64,
) {
    let expected = previous.wrapping_add(1);
    if current == expected {
        return;
    }

    let (kind, missing) = if current == previous {
        stats.duplicate_count += 1;
        ("duplicate", 0)
    } else {
        let forward_distance = current.wrapping_sub(expected);
        if forward_distance < (1_u32 << 31) {
            stats.forward_gap_count += 1;
            stats.missing_sequence_count += u64::from(forward_distance);
            ("forward_gap", u64::from(forward_distance))
        } else {
            stats.regression_count += 1;
            ("regression", 0)
        }
    };

    if stats.samples.len() < 100 {
        stats.samples.push(SequenceDiscontinuity {
            publisher_id: key.0,
            channel_id: key.1,
            previous,
            current,
            timestamp_ns,
            kind,
            missing,
        });
    } else {
        stats.samples_truncated = true;
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use dbn::{
        MboMsg, Metadata, SType, Schema,
        encode::{DbnEncoder, EncodeRecord},
    };

    use super::{SequenceStats, decode_stats, record_discontinuity, stream_mbo};

    #[test]
    fn classifies_sequence_discontinuities_and_wraparound() {
        let mut stats = SequenceStats::default();
        let key = (1, Some(2));
        record_discontinuity(&mut stats, key, 4, 7, 10);
        record_discontinuity(&mut stats, key, 7, 7, 11);
        record_discontinuity(&mut stats, key, 7, 3, 12);
        record_discontinuity(&mut stats, key, u32::MAX, 0, 13);

        assert_eq!(stats.forward_gap_count, 1);
        assert_eq!(stats.missing_sequence_count, 2);
        assert_eq!(stats.duplicate_count, 1);
        assert_eq!(stats.regression_count, 1);
        assert_eq!(stats.samples.len(), 3);
    }

    #[test]
    fn malformed_inputs_return_errors_without_panicking() {
        let root = temp_root("malformed");
        fs::create_dir_all(&root).expect("create malformed fixture directory");

        let corrupt_header = root.join("corrupt-header.dbn");
        fs::write(&corrupt_header, b"not-a-dbn-header").expect("write corrupt header");
        assert!(decode_stats(&corrupt_header).is_err());

        let bad_zstd = root.join("bad-frame.dbn.zst");
        fs::write(&bad_zstd, [0x28, 0xB5, 0x2F, 0xFD, 0xFF, 0x00])
            .expect("write invalid Zstd frame");
        assert!(decode_stats(&bad_zstd).is_err());

        let truncated = root.join("truncated-record.dbn");
        let metadata = Metadata::builder()
            .dataset("GLBX.MDP3")
            .schema(Some(Schema::Mbo))
            .start(1)
            .stype_in(Some(SType::InstrumentId))
            .stype_out(SType::InstrumentId)
            .build();
        let mut dbn_bytes = Vec::new();
        {
            let mut encoder = DbnEncoder::new(&mut dbn_bytes, &metadata).expect("metadata encode");
            encoder
                .encode_record(&MboMsg::default())
                .expect("record encode");
        }
        let metadata_length = u32::from_le_bytes(
            dbn_bytes[4..8]
                .try_into()
                .expect("DBN metadata length bytes"),
        ) as usize;
        let record_start = 8 + metadata_length;
        dbn_bytes.truncate(record_start + std::mem::size_of::<dbn::RecordHeader>());
        fs::write(&truncated, dbn_bytes).expect("write truncated record");
        let mut stream = stream_mbo(&truncated).expect("valid metadata opens");
        assert!(stream.next_record().is_err());

        fs::remove_dir_all(root).expect("remove malformed fixtures");
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dbn-es-core-{label}-{}", std::process::id()))
    }
}
