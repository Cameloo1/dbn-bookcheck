//! Node.js bindings for the streaming DBN core.

#![forbid(unsafe_code)]

use std::{collections::VecDeque, fmt::Display};

use dbn::MboMsg;
use dbn_es_core::{
    BookSet, DecodeStats, SequenceDiscontinuity, SequenceStats, SweepConfig, SweepDetector,
    SweepDirection, SweepEvent, TypedStream, decode_stats as core_decode_stats, stream_mbo,
};
use napi::{Error, Result, Status, bindgen_prelude::BigInt};
use napi_derive::napi;

/// One record-type count from [`JsDecodeStats`].
#[napi(object, object_from_js = false)]
pub struct JsRecordTypeCount {
    /// DBN record type name.
    pub record_type: String,
    /// Records of this type.
    pub count: u64,
}

/// One bounded sequence-discontinuity sample.
#[napi(object, object_from_js = false)]
pub struct JsSequenceDiscontinuity {
    /// Databento publisher identifier.
    pub publisher_id: u32,
    /// MBO channel identifier when present.
    pub channel_id: Option<u32>,
    /// Previous venue sequence number.
    pub previous: u32,
    /// Current venue sequence number.
    pub current: u32,
    /// Current record timestamp in Unix nanoseconds.
    pub timestamp_ns: u64,
    /// `forward_gap`, `duplicate`, or `regression`.
    pub kind: String,
    /// Missing venue sequence values for a forward gap.
    pub missing: u64,
}

/// Sequence diagnostics returned to JavaScript.
#[napi(object, object_from_js = false)]
pub struct JsSequenceStats {
    /// Records that carried sequence numbers.
    pub records_observed: u64,
    /// Forward sequence discontinuities.
    pub forward_gap_count: u64,
    /// Total missing sequence values.
    pub missing_sequence_count: u64,
    /// Repeated sequence values.
    pub duplicate_count: u64,
    /// Backward sequence movements.
    pub regression_count: u64,
    /// First one hundred sampled discontinuities.
    pub samples: Vec<JsSequenceDiscontinuity>,
    /// Whether more samples were omitted.
    pub samples_truncated: bool,
    /// Interpretation warning for filtered symbol streams.
    pub interpretation: String,
}

/// Streaming decode diagnostics returned to JavaScript.
#[napi(object, object_from_js = false)]
pub struct JsDecodeStats {
    /// Input path.
    pub path: String,
    /// DBN metadata schema.
    pub schema: Option<String>,
    /// Total decoded records.
    pub record_count: u64,
    /// Counts by DBN record type.
    pub record_counts_by_type: Vec<JsRecordTypeCount>,
    /// First stream timestamp in Unix nanoseconds.
    pub first_timestamp_ns: Option<u64>,
    /// Last stream timestamp in Unix nanoseconds.
    pub last_timestamp_ns: Option<u64>,
    /// Minimum timestamp in Unix nanoseconds.
    pub min_timestamp_ns: Option<u64>,
    /// Maximum timestamp in Unix nanoseconds.
    pub max_timestamp_ns: Option<u64>,
    /// Records whose timestamp regressed.
    pub timestamp_regression_count: u64,
    /// Sequence diagnostics when the schema carries venue sequence numbers.
    pub sequence: Option<JsSequenceStats>,
}

/// One owned MBO record yielded across the N-API boundary.
#[napi(object, object_from_js = false)]
pub struct JsMboRecord {
    /// Exchange event timestamp in Unix nanoseconds.
    pub timestamp_event_ns: u64,
    /// Capture-server receive timestamp in Unix nanoseconds.
    pub timestamp_receive_ns: u64,
    /// Databento publisher identifier.
    pub publisher_id: u32,
    /// Numeric instrument identifier.
    pub instrument_id: u32,
    /// Venue order identifier.
    pub order_id: u64,
    /// Fixed-point price in units of 1e-9.
    pub price: BigInt,
    /// Order quantity.
    pub size: u32,
    /// Raw DBN flag bitset.
    pub flags: u32,
    /// Channel identifier.
    pub channel_id: u32,
    /// Canonical DBN action character.
    pub action: String,
    /// Canonical DBN side character.
    pub side: String,
    /// Receive-to-send delta.
    pub timestamp_in_delta: i32,
    /// Venue message sequence.
    pub sequence: u32,
}

/// Four-parameter sweep configuration accepted from JavaScript.
#[derive(Clone, Copy)]
#[napi(object)]
pub struct JsSweepConfig {
    /// Number of earlier trades in the rolling extreme.
    pub lookback_trades: u32,
    /// Minimum extreme penetration in ticks.
    pub threshold_ticks: u32,
    /// Maximum reversion window in milliseconds.
    pub reversion_window_ms: u32,
    /// Instrument tick size in DBN fixed-price units.
    pub tick_size: i32,
}

/// One liquidity sweep returned to JavaScript.
#[napi(object, object_from_js = false)]
pub struct JsSweepEvent {
    /// Databento publisher identifier.
    pub publisher_id: u32,
    /// Numeric instrument identifier.
    pub instrument_id: u32,
    /// Emission timestamp in Unix nanoseconds.
    pub timestamp_ns: u64,
    /// Threshold-crossing timestamp in Unix nanoseconds.
    pub sweep_timestamp_ns: u64,
    /// Reversion timestamp in Unix nanoseconds.
    pub reversion_timestamp_ns: u64,
    /// `above_high` or `below_low`.
    pub direction: String,
    /// Swept fixed-point price level.
    pub swept_level: BigInt,
    /// Maximum displacement beyond the level.
    pub displacement_ticks: u64,
    /// Elapsed sweep duration in nanoseconds.
    pub duration_ns: u64,
    /// Visible size consumed at the swept level.
    pub resting_size_consumed: u64,
}

/// Decodes one DBN file and returns the same core statistics used by the Rust CLI.
///
/// # Errors
/// Throws a JavaScript error when the DBN decoder rejects the file.
#[napi]
pub fn decode_stats(path: String) -> Result<JsDecodeStats> {
    core_decode_stats(path)
        .map(JsDecodeStats::from)
        .map_err(to_napi_error)
}

/// File-backed MBO decoder. Each call owns only the returned record; the remaining
/// input stays in the Rust decoder and is never loaded into a JavaScript array.
#[napi]
pub struct MboDecoder {
    stream: TypedStream<MboMsg>,
    records_read: u64,
}

#[napi]
impl MboDecoder {
    /// Opens an MBO DBN or DBN.ZST file.
    ///
    /// # Errors
    /// Throws when the file is malformed or declares another schema.
    #[napi(constructor)]
    pub fn new(path: String) -> Result<Self> {
        Ok(Self {
            stream: stream_mbo(path).map_err(to_napi_error)?,
            records_read: 0,
        })
    }

    /// Returns the next owned record or `null` at end of stream.
    ///
    /// # Errors
    /// Throws when decoding the next record fails.
    #[napi]
    pub fn next_record(&mut self) -> Result<Option<JsMboRecord>> {
        let Some(message) = self.stream.next_record().map_err(to_napi_error)? else {
            return Ok(None);
        };
        let record = JsMboRecord::try_from(message)?;
        self.records_read += 1;
        Ok(Some(record))
    }

    /// Number of records yielded so far.
    #[napi(getter)]
    #[must_use]
    pub fn records_read(&self) -> u64 {
        self.records_read
    }
}

/// Stateful sweep scanner over a file-backed MBO stream and reconstructed book.
/// Each call scans until one event is available or the input ends.
#[napi]
pub struct MboSweepDetector {
    stream: TypedStream<MboMsg>,
    books: BookSet,
    detector: SweepDetector,
    pending: VecDeque<SweepEvent>,
    records_scanned: u64,
}

#[napi]
impl MboSweepDetector {
    /// Opens an MBO stream and validates the four detector parameters.
    ///
    /// # Errors
    /// Throws for invalid parameters, malformed input, or a non-MBO schema.
    #[napi(constructor)]
    pub fn new(path: String, config: JsSweepConfig) -> Result<Self> {
        let config = SweepConfig {
            lookback_trades: usize::try_from(config.lookback_trades).map_err(to_napi_error)?,
            threshold_ticks: config.threshold_ticks,
            reversion_window_ms: u64::from(config.reversion_window_ms),
            tick_size: i64::from(config.tick_size),
        };
        Ok(Self {
            stream: stream_mbo(path).map_err(to_napi_error)?,
            books: BookSet::default(),
            detector: SweepDetector::new(config).map_err(to_napi_error)?,
            pending: VecDeque::new(),
            records_scanned: 0,
        })
    }

    /// Returns the next completed sweep or `null` at end of stream.
    ///
    /// # Errors
    /// Throws on malformed DBN, invalid action bytes, timestamp regression, or an
    /// inconsistent order-book transition.
    #[napi]
    pub fn next_sweep(&mut self) -> Result<Option<JsSweepEvent>> {
        if let Some(event) = self.pending.pop_front() {
            return Ok(Some(JsSweepEvent::from(event)));
        }
        loop {
            let Some(message) = self.stream.next_record().map_err(to_napi_error)? else {
                return Ok(None);
            };
            self.records_scanned += 1;
            let events = self
                .detector
                .observe(message, &self.books)
                .map_err(to_napi_error)?;
            self.books.apply(message).map_err(to_napi_error)?;
            self.pending.extend(events);
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(JsSweepEvent::from(event)));
            }
        }
    }

    /// Number of MBO records scanned so far.
    #[napi(getter)]
    #[must_use]
    pub fn records_scanned(&self) -> u64 {
        self.records_scanned
    }
}

impl TryFrom<&MboMsg> for JsMboRecord {
    type Error = Error;

    fn try_from(message: &MboMsg) -> Result<Self> {
        let action = message.action().map_err(to_napi_error)?;
        let side = message.side().map_err(to_napi_error)?;
        Ok(Self {
            timestamp_event_ns: message.hd.ts_event,
            timestamp_receive_ns: message.ts_recv,
            publisher_id: u32::from(message.hd.publisher_id),
            instrument_id: message.hd.instrument_id,
            order_id: message.order_id,
            price: message.price.into(),
            size: message.size,
            flags: u32::from(message.flags.raw()),
            channel_id: u32::from(message.channel_id),
            action: char::from(action).to_string(),
            side: char::from(side).to_string(),
            timestamp_in_delta: message.ts_in_delta,
            sequence: message.sequence,
        })
    }
}

impl From<DecodeStats> for JsDecodeStats {
    fn from(stats: DecodeStats) -> Self {
        Self {
            path: stats.path.to_string_lossy().into_owned(),
            schema: stats.schema,
            record_count: stats.record_count,
            record_counts_by_type: stats
                .record_counts_by_type
                .into_iter()
                .map(|(record_type, count)| JsRecordTypeCount { record_type, count })
                .collect(),
            first_timestamp_ns: stats.first_timestamp_ns,
            last_timestamp_ns: stats.last_timestamp_ns,
            min_timestamp_ns: stats.min_timestamp_ns,
            max_timestamp_ns: stats.max_timestamp_ns,
            timestamp_regression_count: stats.timestamp_regression_count,
            sequence: stats.sequence.map(JsSequenceStats::from),
        }
    }
}

impl From<SequenceStats> for JsSequenceStats {
    fn from(stats: SequenceStats) -> Self {
        Self {
            records_observed: stats.records_observed,
            forward_gap_count: stats.forward_gap_count,
            missing_sequence_count: stats.missing_sequence_count,
            duplicate_count: stats.duplicate_count,
            regression_count: stats.regression_count,
            samples: stats
                .samples
                .into_iter()
                .map(JsSequenceDiscontinuity::from)
                .collect(),
            samples_truncated: stats.samples_truncated,
            interpretation: stats.interpretation.to_owned(),
        }
    }
}

impl From<SequenceDiscontinuity> for JsSequenceDiscontinuity {
    fn from(sample: SequenceDiscontinuity) -> Self {
        Self {
            publisher_id: u32::from(sample.publisher_id),
            channel_id: sample.channel_id.map(u32::from),
            previous: sample.previous,
            current: sample.current,
            timestamp_ns: sample.timestamp_ns,
            kind: sample.kind.to_owned(),
            missing: sample.missing,
        }
    }
}

impl From<SweepEvent> for JsSweepEvent {
    fn from(event: SweepEvent) -> Self {
        Self {
            publisher_id: u32::from(event.instrument.publisher_id),
            instrument_id: event.instrument.instrument_id,
            timestamp_ns: event.timestamp_ns,
            sweep_timestamp_ns: event.sweep_timestamp_ns,
            reversion_timestamp_ns: event.reversion_timestamp_ns,
            direction: match event.direction {
                SweepDirection::AboveHigh => "above_high",
                SweepDirection::BelowLow => "below_low",
            }
            .to_owned(),
            swept_level: event.swept_level.into(),
            displacement_ticks: event.displacement_ticks,
            duration_ns: event.duration_ns,
            resting_size_consumed: event.resting_size_consumed,
        }
    }
}

fn to_napi_error(error: impl Display) -> Error {
    Error::new(Status::GenericFailure, error.to_string())
}
