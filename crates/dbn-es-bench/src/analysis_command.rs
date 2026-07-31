use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap, VecDeque},
    fmt::Write as FmtWrite,
    fs::{self, File},
    io::{self, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use dbn::{Action, MboMsg, Mbp10Msg, UNDEF_PRICE};
use dbn_es_core::{
    BookError, BookLevel, BookSet, DecodeError, InstrumentKey, SweepConfig, SweepDetector,
    SweepDirection, SweepError, TopOfBook, TypedStream, stream_mbo, stream_mbp10,
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
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
    Decode(#[from] DecodeError),
    #[error(transparent)]
    Book(#[from] BookError),
    #[error(transparent)]
    Sweep(#[from] SweepError),
    #[error("manifest validation failed: {0}")]
    Manifest(String),
    #[error("analysis invariant failed: {0}")]
    Invariant(String),
    #[error("failed to format report timestamp: {0}")]
    Timestamp(#[from] time::error::Format),
    #[error("failed to render report")]
    Render,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestEntry {
    path: PathBuf,
    schema: String,
    source: String,
    dataset: String,
    symbol: String,
    start: String,
    end: String,
    record_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct AlignmentKey {
    timestamp_ns: u64,
    publisher_id: u16,
    instrument_id: u32,
    sequence: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecordIdentity {
    action: char,
    price: i64,
    size: u32,
}

#[derive(Debug, Clone, Serialize)]
struct MboContext {
    key: AlignmentKey,
    order_id: u64,
    price: i64,
    size: u32,
    flags: u8,
    channel_id: u8,
    action: char,
    side: char,
    ts_recv: u64,
    ts_in_delta: i32,
}

impl From<&MboMsg> for MboContext {
    fn from(message: &MboMsg) -> Self {
        Self {
            key: AlignmentKey {
                timestamp_ns: message.hd.ts_event,
                publisher_id: message.hd.publisher_id,
                instrument_id: message.hd.instrument_id,
                sequence: message.sequence,
            },
            order_id: message.order_id,
            price: message.price,
            size: message.size,
            flags: message.flags.raw(),
            channel_id: message.channel_id,
            action: u8::try_from(message.action).map_or('\u{fffd}', char::from),
            side: u8::try_from(message.side).map_or('\u{fffd}', char::from),
            ts_recv: message.ts_recv,
            ts_in_delta: message.ts_in_delta,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
struct MbpLevelContext {
    depth: usize,
    bid_px: i64,
    ask_px: i64,
    bid_sz: u32,
    ask_sz: u32,
    bid_ct: u32,
    ask_ct: u32,
}

#[derive(Debug, Clone, Serialize)]
struct MbpContext {
    key: AlignmentKey,
    price: i64,
    size: u32,
    flags: u8,
    action: char,
    side: char,
    depth: u8,
    ts_recv: u64,
    ts_in_delta: i32,
    levels: Vec<MbpLevelContext>,
}

impl From<&Mbp10Msg> for MbpContext {
    fn from(message: &Mbp10Msg) -> Self {
        Self {
            key: AlignmentKey {
                timestamp_ns: message.hd.ts_event,
                publisher_id: message.hd.publisher_id,
                instrument_id: message.hd.instrument_id,
                sequence: message.sequence,
            },
            price: message.price,
            size: message.size,
            flags: message.flags.raw(),
            action: u8::try_from(message.action).map_or('\u{fffd}', char::from),
            side: u8::try_from(message.side).map_or('\u{fffd}', char::from),
            depth: message.depth,
            ts_recv: message.ts_recv,
            ts_in_delta: message.ts_in_delta,
            levels: message
                .levels
                .iter()
                .enumerate()
                .map(|(depth, level)| MbpLevelContext {
                    depth,
                    bid_px: level.bid_px,
                    ask_px: level.ask_px,
                    bid_sz: level.bid_sz,
                    ask_sz: level.ask_sz,
                    bid_ct: level.bid_ct,
                    ask_ct: level.ask_ct,
                })
                .collect(),
        }
    }
}

impl MbpContext {
    fn identity(&self) -> RecordIdentity {
        RecordIdentity {
            action: self.action,
            price: self.price,
            size: self.size,
        }
    }

    fn top(&self) -> TopOfBook {
        let first = self.levels.first();
        TopOfBook {
            bid: first.and_then(|level| {
                (level.bid_px != UNDEF_PRICE).then_some(BookLevel {
                    price: level.bid_px,
                    size: u64::from(level.bid_sz),
                    order_count: level.bid_ct,
                })
            }),
            ask: first.and_then(|level| {
                (level.ask_px != UNDEF_PRICE).then_some(BookLevel {
                    price: level.ask_px,
                    size: u64::from(level.ask_sz),
                    order_count: level.ask_ct,
                })
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ReconstructedObservation {
    key: AlignmentKey,
    top: TopOfBook,
    context: MboContext,
}

impl ReconstructedObservation {
    fn identity(&self) -> RecordIdentity {
        RecordIdentity {
            action: self.context.action,
            price: self.context.price,
            size: self.context.size,
        }
    }
}

struct MboEvents {
    stream: TypedStream<MboMsg>,
    books: BookSet,
    pending: HashMap<InstrumentKey, PendingEvent>,
    ready: VecDeque<ReconstructedObservation>,
    records_scanned: u64,
    invalid_records_withheld: u64,
    invalid_event_boundaries: u64,
    valid_event_boundaries: u64,
}

struct PendingObservation {
    context: MboContext,
    is_trade: bool,
}

struct PendingEvent {
    top_before: Option<TopOfBook>,
    observations: Vec<PendingObservation>,
}

impl MboEvents {
    fn new(path: &Path) -> Result<Self, AnalysisError> {
        Ok(Self {
            stream: stream_mbo(path)?,
            books: BookSet::default(),
            pending: HashMap::new(),
            ready: VecDeque::new(),
            records_scanned: 0,
            invalid_records_withheld: 0,
            invalid_event_boundaries: 0,
            valid_event_boundaries: 0,
        })
    }

    fn next_observation(&mut self) -> Result<Option<ReconstructedObservation>, AnalysisError> {
        if let Some(observation) = self.ready.pop_front() {
            return Ok(Some(observation));
        }
        while let Some(message) = self.stream.next_record()? {
            self.records_scanned += 1;
            let context = MboContext::from(message);
            let action = message.action().map_err(|_| {
                AnalysisError::Invariant(format!(
                    "invalid MBO action on instrument {} at {}",
                    message.hd.instrument_id, message.hd.ts_event
                ))
            })?;
            let instrument = InstrumentKey::from(message);
            let top_before = self.books.top(instrument);
            self.pending.entry(instrument).or_insert(PendingEvent {
                top_before,
                observations: Vec::new(),
            });
            let update = self.books.apply(message)?;
            if (action == Action::Trade || update.state_changed)
                && self
                    .pending
                    .get(&instrument)
                    .is_some_and(|event| event.top_before.is_some())
                && let Some(event) = self.pending.get_mut(&instrument)
            {
                event.observations.push(PendingObservation {
                    context,
                    is_trade: action == Action::Trade,
                });
            }
            if !update.book_valid {
                self.invalid_records_withheld += 1;
                if update.event_complete {
                    self.invalid_event_boundaries += 1;
                }
            }
            if update.event_complete {
                if update.book_valid {
                    self.valid_event_boundaries += 1;
                }
                let pending = self.pending.remove(&instrument).ok_or_else(|| {
                    AnalysisError::Invariant(format!(
                        "event state missing for instrument {} at {}",
                        instrument.instrument_id, message.hd.ts_event
                    ))
                })?;
                if let (Some(pre_event_top), Some(final_top)) = (pending.top_before, update.top) {
                    for observation in pending.observations {
                        self.ready.push_back(ReconstructedObservation {
                            key: observation.context.key,
                            top: if observation.is_trade {
                                pre_event_top
                            } else {
                                final_top
                            },
                            context: observation.context,
                        });
                    }
                }
                if let Some(observation) = self.ready.pop_front() {
                    return Ok(Some(observation));
                }
            }
        }
        Ok(None)
    }
}

struct MbpUpdates {
    stream: TypedStream<Mbp10Msg>,
    records_scanned: u64,
}

impl MbpUpdates {
    fn new(path: &Path) -> Result<Self, AnalysisError> {
        Ok(Self {
            stream: stream_mbp10(path)?,
            records_scanned: 0,
        })
    }

    fn next_update(&mut self) -> Result<Option<MbpContext>, AnalysisError> {
        let Some(message) = self.stream.next_record()? else {
            return Ok(None);
        };
        self.records_scanned += 1;
        Ok(Some(MbpContext::from(message)))
    }
}

fn next_reconstructed_group(
    source: &mut MboEvents,
    cursor: &mut Option<ReconstructedObservation>,
) -> Result<Vec<ReconstructedObservation>, AnalysisError> {
    let Some(first) = cursor
        .take()
        .map_or_else(|| source.next_observation(), |value| Ok(Some(value)))?
    else {
        return Ok(Vec::new());
    };
    let key = first.key;
    let mut group = vec![first];
    while let Some(next) = source.next_observation()? {
        if next.key == key {
            group.push(next);
        } else {
            *cursor = Some(next);
            break;
        }
    }
    Ok(group)
}

fn next_exchange_group(
    source: &mut MbpUpdates,
    cursor: &mut Option<MbpContext>,
) -> Result<Vec<MbpContext>, AnalysisError> {
    let Some(first) = cursor
        .take()
        .map_or_else(|| source.next_update(), |value| Ok(Some(value)))?
    else {
        return Ok(Vec::new());
    };
    let key = first.key;
    let mut group = vec![first];
    while let Some(next) = source.next_update()? {
        if next.key == key {
            group.push(next);
        } else {
            *cursor = Some(next);
            break;
        }
    }
    Ok(group)
}

#[derive(Debug, Clone, Serialize)]
struct Discrepancy {
    key: AlignmentKey,
    classification: String,
    reconstructed: TopOfBook,
    exchange_mbp10: TopOfBook,
    mbo_context: MboContext,
    mbp10_context: MbpContext,
}

#[derive(Debug, Serialize)]
pub struct ValidationReport {
    version: u32,
    generated_at: String,
    source: String,
    dataset: String,
    symbol: String,
    start: String,
    end: String,
    tick_size: i64,
    methodology: String,
    mbo_records_scanned: u64,
    mbp10_records_scanned: u64,
    invalid_mbo_records_withheld_before_baseline_or_recovery: u64,
    invalid_mbo_event_boundaries: u64,
    valid_mbo_event_boundaries: u64,
    mbp10_updates_before_valid_mbo_baseline: u64,
    aligned_updates: u64,
    unmatched_mbo_observations: u64,
    unmatched_mbp10_updates: u64,
    exact_matches: u64,
    price_matches: u64,
    size_matches: u64,
    aligned_exact_match_pct: f64,
    end_to_end_exact_match_pct: f64,
    alignment_coverage_pct: f64,
    discrepancy_histogram: BTreeMap<String, u64>,
    first_ten_discrepancies: Vec<Discrepancy>,
}

#[allow(clippy::too_many_lines)]
pub fn run_validation(
    manifest_path: &Path,
    markdown_path: &Path,
    json_path: &Path,
    tick_size: i64,
) -> Result<ValidationReport, AnalysisError> {
    if tick_size <= 0 {
        return Err(AnalysisError::Manifest(
            "validation tick size must be positive".to_owned(),
        ));
    }
    let manifest = read_json::<Manifest>(manifest_path)?;
    let (order_entry, depth_entry) = aligned_entries(&manifest)?;
    let mut reconstructed = MboEvents::new(&order_entry.path)?;
    let mut exchange = MbpUpdates::new(&depth_entry.path)?;
    let mut reconstructed_cursor = None;
    let mut exchange_cursor = None;
    let mut reconstructed_group =
        next_reconstructed_group(&mut reconstructed, &mut reconstructed_cursor)?;
    let mut exchange_group = next_exchange_group(&mut exchange, &mut exchange_cursor)?;
    let mut mbp_before_baseline = 0_u64;
    let mut aligned = 0_u64;
    let mut unmatched_reconstructed = 0_u64;
    let mut unmatched_exchange = 0_u64;
    let mut exact = 0_u64;
    let mut price_matches = 0_u64;
    let mut size_matches = 0_u64;
    let mut histogram = BTreeMap::new();
    let mut discrepancies = Vec::new();

    let baseline_key = reconstructed_group.first().map(|event| event.key);
    if let Some(baseline) = baseline_key {
        while exchange_group
            .first()
            .is_some_and(|update| update.key < baseline)
        {
            mbp_before_baseline += exchange_group.len() as u64;
            exchange_group = next_exchange_group(&mut exchange, &mut exchange_cursor)?;
        }
    }

    while let (Some(order_key), Some(depth_key)) = (
        reconstructed_group.first().map(|event| event.key),
        exchange_group.first().map(|update| update.key),
    ) {
        match order_key.cmp(&depth_key) {
            Ordering::Less => {
                unmatched_reconstructed += reconstructed_group.len() as u64;
                reconstructed_group =
                    next_reconstructed_group(&mut reconstructed, &mut reconstructed_cursor)?;
            }
            Ordering::Greater => {
                unmatched_exchange += exchange_group.len() as u64;
                exchange_group = next_exchange_group(&mut exchange, &mut exchange_cursor)?;
            }
            Ordering::Equal => {
                let mut used = vec![false; reconstructed_group.len()];
                for depth_update in &exchange_group {
                    let matched_index =
                        reconstructed_group
                            .iter()
                            .enumerate()
                            .position(|(index, observation)| {
                                !used[index] && observation.identity() == depth_update.identity()
                            });
                    if let Some(index) = matched_index {
                        used[index] = true;
                        let order_event = &reconstructed_group[index];
                        aligned += 1;
                        let exchange_top = depth_update.top();
                        let (price_match, size_match) =
                            component_matches(order_event.top, exchange_top);
                        price_matches += u64::from(price_match);
                        size_matches += u64::from(size_match);
                        if price_match && size_match {
                            exact += 1;
                        } else {
                            let class = discrepancy_class(order_event.top, exchange_top, tick_size);
                            *histogram.entry(class.clone()).or_default() += 1;
                            if discrepancies.len() < 10 {
                                discrepancies.push(Discrepancy {
                                    key: order_event.key,
                                    classification: class,
                                    reconstructed: order_event.top,
                                    exchange_mbp10: exchange_top,
                                    mbo_context: order_event.context.clone(),
                                    mbp10_context: depth_update.clone(),
                                });
                            }
                        }
                    } else {
                        unmatched_exchange += 1;
                    }
                }
                unmatched_reconstructed +=
                    used.iter().filter(|was_used| !**was_used).count() as u64;
                reconstructed_group =
                    next_reconstructed_group(&mut reconstructed, &mut reconstructed_cursor)?;
                exchange_group = next_exchange_group(&mut exchange, &mut exchange_cursor)?;
            }
        }
    }
    while !reconstructed_group.is_empty() {
        unmatched_reconstructed += reconstructed_group.len() as u64;
        reconstructed_group =
            next_reconstructed_group(&mut reconstructed, &mut reconstructed_cursor)?;
    }
    while !exchange_group.is_empty() {
        unmatched_exchange += exchange_group.len() as u64;
        exchange_group = next_exchange_group(&mut exchange, &mut exchange_cursor)?;
    }

    if reconstructed.records_scanned != order_entry.record_count {
        return Err(AnalysisError::Invariant(format!(
            "MBO manifest count {} differs from analyzed count {}",
            order_entry.record_count, reconstructed.records_scanned
        )));
    }
    if exchange.records_scanned != depth_entry.record_count {
        return Err(AnalysisError::Invariant(format!(
            "MBP-10 manifest count {} differs from analyzed count {}",
            depth_entry.record_count, exchange.records_scanned
        )));
    }

    histogram.insert("exact".to_owned(), exact);
    let eligible_mbp = aligned + unmatched_exchange;
    let report = ValidationReport {
        version: 1,
        generated_at: now_rfc3339()?,
        source: order_entry.source.clone(),
        dataset: order_entry.dataset.clone(),
        symbol: order_entry.symbol.clone(),
        start: order_entry.start.clone(),
        end: order_entry.end.clone(),
        tick_size,
        methodology: "Streaming merge groups records by exact (ts_event, publisher_id, instrument_id, sequence), then pairs only identical action, price, and size records within each group. Each MBO event is buffered through F_LAST: Trade records compare the reconstructed pre-event book, while state-changing Add, Cancel, Modify, and Clear records compare the final event book. MBO state is withheld until a complete clear/snapshot ending in F_LAST. Every MBP-10 update after that baseline remains in the end-to-end denominator; unmatched records are failures, not dropped observations.".to_owned(),
        mbo_records_scanned: reconstructed.records_scanned,
        mbp10_records_scanned: exchange.records_scanned,
        invalid_mbo_records_withheld_before_baseline_or_recovery: reconstructed
            .invalid_records_withheld,
        invalid_mbo_event_boundaries: reconstructed.invalid_event_boundaries,
        valid_mbo_event_boundaries: reconstructed.valid_event_boundaries,
        mbp10_updates_before_valid_mbo_baseline: mbp_before_baseline,
        aligned_updates: aligned,
        unmatched_mbo_observations: unmatched_reconstructed,
        unmatched_mbp10_updates: unmatched_exchange,
        exact_matches: exact,
        price_matches,
        size_matches,
        aligned_exact_match_pct: percentage(exact, aligned),
        end_to_end_exact_match_pct: percentage(exact, eligible_mbp),
        alignment_coverage_pct: percentage(aligned, eligible_mbp),
        discrepancy_histogram: histogram,
        first_ten_discrepancies: discrepancies,
    };
    write_json_atomic(json_path, &report)?;
    write_text_atomic(markdown_path, &render_validation_markdown(&report)?)?;
    Ok(report)
}

fn aligned_entries(manifest: &Manifest) -> Result<(&ManifestEntry, &ManifestEntry), AnalysisError> {
    let order_entry = one_schema(manifest, "mbo")?;
    let depth_entry = one_schema(manifest, "mbp-10")?;
    for (field, left, right) in [
        (
            "source",
            order_entry.source.as_str(),
            depth_entry.source.as_str(),
        ),
        (
            "dataset",
            order_entry.dataset.as_str(),
            depth_entry.dataset.as_str(),
        ),
        (
            "symbol",
            order_entry.symbol.as_str(),
            depth_entry.symbol.as_str(),
        ),
        (
            "start",
            order_entry.start.as_str(),
            depth_entry.start.as_str(),
        ),
        ("end", order_entry.end.as_str(), depth_entry.end.as_str()),
    ] {
        if left != right {
            return Err(AnalysisError::Manifest(format!(
                "MBO and MBP-10 {field} differ: {left:?} vs {right:?}"
            )));
        }
    }
    if !matches!(order_entry.source.as_str(), "live" | "synthetic") {
        return Err(AnalysisError::Manifest(format!(
            "aligned validation source must be live or synthetic, found {:?}",
            order_entry.source
        )));
    }
    Ok((order_entry, depth_entry))
}

fn one_schema<'a>(
    manifest: &'a Manifest,
    schema: &str,
) -> Result<&'a ManifestEntry, AnalysisError> {
    let mut matches = manifest
        .entries
        .iter()
        .filter(|entry| entry.schema == schema);
    let entry = matches
        .next()
        .ok_or_else(|| AnalysisError::Manifest(format!("missing {schema} entry")))?;
    if matches.next().is_some() {
        return Err(AnalysisError::Manifest(format!(
            "multiple {schema} entries are ambiguous"
        )));
    }
    Ok(entry)
}

fn component_matches(reconstructed: TopOfBook, exchange: TopOfBook) -> (bool, bool) {
    let price = reconstructed.bid.map(|level| level.price) == exchange.bid.map(|level| level.price)
        && reconstructed.ask.map(|level| level.price) == exchange.ask.map(|level| level.price);
    let size = reconstructed.bid.map(|level| level.size) == exchange.bid.map(|level| level.size)
        && reconstructed.ask.map(|level| level.size) == exchange.ask.map(|level| level.size);
    (price, size)
}

fn discrepancy_class(reconstructed: TopOfBook, exchange: TopOfBook, tick_size: i64) -> String {
    if reconstructed.bid.is_none()
        || reconstructed.ask.is_none()
        || exchange.bid.is_none()
        || exchange.ask.is_none()
    {
        return "missing_side".to_owned();
    }
    let price_ticks = [
        price_delta_ticks(reconstructed.bid, exchange.bid, tick_size),
        price_delta_ticks(reconstructed.ask, exchange.ask, tick_size),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    match price_ticks {
        0 => "size_only".to_owned(),
        1 => "price_1_tick".to_owned(),
        2 => "price_2_ticks".to_owned(),
        3..=5 => "price_3_to_5_ticks".to_owned(),
        6..=10 => "price_6_to_10_ticks".to_owned(),
        _ => "price_over_10_ticks".to_owned(),
    }
}

fn price_delta_ticks(left: Option<BookLevel>, right: Option<BookLevel>, tick_size: i64) -> u64 {
    match (left, right) {
        (Some(left), Some(right)) => left.price.abs_diff(right.price) / tick_size.unsigned_abs(),
        _ => u64::MAX,
    }
}

#[allow(clippy::cast_precision_loss)]
fn percentage(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 * 100.0 / denominator as f64
    }
}

#[allow(clippy::too_many_lines)]
fn render_validation_markdown(report: &ValidationReport) -> Result<String, AnalysisError> {
    let mut output = String::new();
    writeln!(output, "# Book reconstruction validation").map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "**Result:** {:.6}% end-to-end exact top-of-book match and {:.6}% exact match among precisely aligned updates, measured on {} `{}` data.",
        report.end_to_end_exact_match_pct,
        report.aligned_exact_match_pct,
        report.source,
        report.symbol
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "## Provenance and method").map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "- Dataset: `{}`; source: `{}`; interval: `{}` through `{}`.",
        report.dataset, report.source, report.start, report.end
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "- Tick size: `{}` fixed-price units.",
        report.tick_size
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(output, "- {}", report.methodology).map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "## Measured coverage and accuracy").map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "| Metric | Value |").map_err(|_| AnalysisError::Render)?;
    writeln!(output, "| --- | ---: |").map_err(|_| AnalysisError::Render)?;
    for (label, value) in [
        ("MBO records scanned", report.mbo_records_scanned),
        ("MBP-10 records scanned", report.mbp10_records_scanned),
        (
            "MBO records withheld before baseline/recovery",
            report.invalid_mbo_records_withheld_before_baseline_or_recovery,
        ),
        (
            "MBP-10 updates before valid MBO baseline",
            report.mbp10_updates_before_valid_mbo_baseline,
        ),
        ("Aligned updates", report.aligned_updates),
        (
            "Unmatched eligible MBO observations",
            report.unmatched_mbo_observations,
        ),
        ("Unmatched MBP-10 updates", report.unmatched_mbp10_updates),
        ("Exact price-and-size matches", report.exact_matches),
        ("Price matches", report.price_matches),
        ("Size matches", report.size_matches),
    ] {
        writeln!(output, "| {label} | {value} |").map_err(|_| AnalysisError::Render)?;
    }
    writeln!(
        output,
        "| Alignment coverage | {:.6}% |",
        report.alignment_coverage_pct
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "| Aligned exact match | {:.6}% |",
        report.aligned_exact_match_pct
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "| End-to-end exact match | {:.6}% |",
        report.end_to_end_exact_match_pct
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "## Discrepancy histogram").map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "| Classification | Updates |").map_err(|_| AnalysisError::Render)?;
    writeln!(output, "| --- | ---: |").map_err(|_| AnalysisError::Render)?;
    for (class, count) in &report.discrepancy_histogram {
        writeln!(output, "| `{class}` | {count} |").map_err(|_| AnalysisError::Render)?;
    }
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "## Failure-mode analysis").map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "The request begins mid-session, so reconstruction is intentionally withheld until Databento's complete daily MBO snapshot establishes a baseline. That is a correctness boundary, not a tuned exclusion. Exact-key misses after the baseline remain in the end-to-end denominator. MBO events are held through F_LAST because intermediate book state is not inspectable; MBP-10 trade snapshots use the pre-event book while mutations use final event state. Aligned price mismatches indicate reconstruction or source-semantic disagreement; size-only mismatches indicate aggregation disagreement even when prices align. Venue sequence values may repeat across normalized records, so alignment also requires timestamp, publisher, instrument, and record identity."
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "## First ten aligned discrepancies").map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    if report.first_ten_discrepancies.is_empty() {
        writeln!(output, "No aligned discrepancies were observed.")
            .map_err(|_| AnalysisError::Render)?;
    } else {
        for (index, discrepancy) in report.first_ten_discrepancies.iter().enumerate() {
            writeln!(output, "### Discrepancy {}", index + 1).map_err(|_| AnalysisError::Render)?;
            writeln!(output).map_err(|_| AnalysisError::Render)?;
            writeln!(output, "```json").map_err(|_| AnalysisError::Render)?;
            let json = serde_json::to_string_pretty(discrepancy).map_err(|source| {
                AnalysisError::Json {
                    path: PathBuf::from("<book-validation-render>"),
                    source,
                }
            })?;
            writeln!(output, "{json}").map_err(|_| AnalysisError::Render)?;
            writeln!(output, "```").map_err(|_| AnalysisError::Render)?;
            writeln!(output).map_err(|_| AnalysisError::Render)?;
        }
    }
    Ok(output)
}

#[derive(Debug, Serialize)]
pub struct SweepSummary {
    version: u32,
    generated_at: String,
    source: String,
    dataset: String,
    symbol: String,
    start: String,
    end: String,
    config: SweepConfig,
    mbo_records_scanned: u64,
    event_count: u64,
    above_high_count: u64,
    below_low_count: u64,
    first_event_timestamp_ns: Option<u64>,
    last_event_timestamp_ns: Option<u64>,
    output_path: PathBuf,
}

pub fn run_sweeps(
    manifest_path: &Path,
    config_path: &Path,
    output_path: &Path,
    summary_path: &Path,
    report_path: &Path,
) -> Result<SweepSummary, AnalysisError> {
    let manifest = read_json::<Manifest>(manifest_path)?;
    let entry = one_schema(&manifest, "mbo")?;
    let config = read_json::<SweepConfig>(config_path)?.validate()?;
    let mut stream = stream_mbo(&entry.path)?;
    let mut books = BookSet::default();
    let mut detector = SweepDetector::new(config)?;
    let part_path = part_path(output_path);
    create_parent(&part_path)?;
    let file = File::create(&part_path).map_err(|source| AnalysisError::Io {
        path: part_path.clone(),
        source,
    })?;
    let mut writer = BufWriter::new(file);
    let mut records = 0_u64;
    let mut event_count = 0_u64;
    let mut above = 0_u64;
    let mut below = 0_u64;
    let mut first_timestamp = None;
    let mut last_timestamp = None;

    while let Some(message) = stream.next_record()? {
        records += 1;
        for event in detector.observe(message, &books)? {
            serde_json::to_writer(&mut writer, &event).map_err(|source| AnalysisError::Json {
                path: part_path.clone(),
                source,
            })?;
            writer
                .write_all(b"\n")
                .map_err(|source| AnalysisError::Io {
                    path: part_path.clone(),
                    source,
                })?;
            event_count += 1;
            match event.direction {
                SweepDirection::AboveHigh => above += 1,
                SweepDirection::BelowLow => below += 1,
            }
            first_timestamp.get_or_insert(event.timestamp_ns);
            last_timestamp = Some(event.timestamp_ns);
        }
        books.apply(message)?;
    }
    if records != entry.record_count {
        return Err(AnalysisError::Invariant(format!(
            "MBO manifest count {} differs from sweep scan count {records}",
            entry.record_count
        )));
    }
    writer.flush().map_err(|source| AnalysisError::Io {
        path: part_path.clone(),
        source,
    })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| AnalysisError::Io {
            path: part_path.clone(),
            source,
        })?;
    replace_with_part(&part_path, output_path)?;

    let summary = SweepSummary {
        version: 1,
        generated_at: now_rfc3339()?,
        source: entry.source.clone(),
        dataset: entry.dataset.clone(),
        symbol: entry.symbol.clone(),
        start: entry.start.clone(),
        end: entry.end.clone(),
        config,
        mbo_records_scanned: records,
        event_count,
        above_high_count: above,
        below_low_count: below,
        first_event_timestamp_ns: first_timestamp,
        last_event_timestamp_ns: last_timestamp,
        output_path: output_path.to_path_buf(),
    };
    write_json_atomic(summary_path, &summary)?;
    write_text_atomic(report_path, &render_sweep_markdown(&summary)?)?;
    Ok(summary)
}

fn render_sweep_markdown(summary: &SweepSummary) -> Result<String, AnalysisError> {
    let mut output = String::new();
    let tick_points = format_fixed_price(summary.config.tick_size);
    writeln!(output, "# Liquidity sweep detection").map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "This detector is a transparent market-data heuristic, not a trading signal or an execution recommendation. It runs on MBO trade records only after the reconstructed book has a complete clear/snapshot baseline.").map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "## Parameters").map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "The committed parameter set is `config/sweep.json` and has four free parameters."
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "| Parameter | Value | Meaning |").map_err(|_| AnalysisError::Render)?;
    writeln!(output, "| --- | ---: | --- |").map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "| `lookback_trades` | {} | Prior trades defining the rolling swing high and low |",
        summary.config.lookback_trades
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "| `threshold_ticks` | {} | Required penetration beyond the prior extreme |",
        summary.config.threshold_ticks
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "| `reversion_window_ms` | {} | Maximum time to trade back through the swept level |",
        summary.config.reversion_window_ms
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "| `tick_size` | {} | ES tick size in DBN fixed-price units ({tick_points} index points) |",
        summary.config.tick_size,
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "For each publisher-qualified instrument, a trade at least {} ticks beyond the prior {}-trade high or low opens at most one candidate in that direction. A later trade must cross back through the swept level within {} milliseconds. The emitted event records the initial sweep time, reversion time, maximum displacement, duration, swept level, and visible resting size attributable to the triggering trade.", summary.config.threshold_ticks, summary.config.lookback_trades, summary.config.reversion_window_ms).map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "`resting_size_consumed` is deliberately conservative: it is the smaller of the pre-event displayed quantity at the swept level and the triggering trade quantity. It does not estimate hidden liquidity or claim that every contract in a multi-level execution was consumed at that one level.").map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "## Verified run").map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "- Provenance: `{}`; dataset `{}`; symbol `{}`; interval `{}` through `{}`.",
        summary.source, summary.dataset, summary.symbol, summary.start, summary.end
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "- Scanned {} MBO records.",
        summary.mbo_records_scanned
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "- Emitted {} monotonic events: {} above prior highs and {} below prior lows.",
        summary.event_count, summary.above_high_count, summary.below_low_count
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(
        output,
        "The reproducible JSONL output is `{}` and remains a generated local artifact.",
        summary.output_path.display()
    )
    .map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "Run it with:").map_err(|_| AnalysisError::Render)?;
    writeln!(output).map_err(|_| AnalysisError::Render)?;
    writeln!(output, "```sh").map_err(|_| AnalysisError::Render)?;
    writeln!(output, "cargo run --release --locked -p dbn-es-bench --bin dbn-es-bench -- analyze sweeps --manifest data/manifest.json --config config/sweep.json --output out/sweeps.jsonl --summary data/sweep-summary.json --report docs/sweep-detection.md").map_err(|_| AnalysisError::Render)?;
    writeln!(output, "```").map_err(|_| AnalysisError::Render)?;
    Ok(output)
}

fn format_fixed_price(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let absolute = value.unsigned_abs();
    let whole = absolute / 1_000_000_000;
    let mut fraction = format!("{:09}", absolute % 1_000_000_000);
    while fraction.ends_with('0') {
        fraction.pop();
    }
    if fraction.is_empty() {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction}")
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, AnalysisError> {
    let file = File::open(path).map_err(|source| AnalysisError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_reader(BufReader::new(file)).map_err(|source| AnalysisError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), AnalysisError> {
    let part = part_path(path);
    create_parent(&part)?;
    let file = File::create(&part).map_err(|source| AnalysisError::Io {
        path: part.clone(),
        source,
    })?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value).map_err(|source| AnalysisError::Json {
        path: part.clone(),
        source,
    })?;
    writer
        .write_all(b"\n")
        .map_err(|source| AnalysisError::Io {
            path: part.clone(),
            source,
        })?;
    writer.flush().map_err(|source| AnalysisError::Io {
        path: part.clone(),
        source,
    })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| AnalysisError::Io {
            path: part.clone(),
            source,
        })?;
    replace_with_part(&part, path)
}

fn write_text_atomic(path: &Path, value: &str) -> Result<(), AnalysisError> {
    let part = part_path(path);
    create_parent(&part)?;
    let mut file = File::create(&part).map_err(|source| AnalysisError::Io {
        path: part.clone(),
        source,
    })?;
    file.write_all(value.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|source| AnalysisError::Io {
            path: part.clone(),
            source,
        })?;
    replace_with_part(&part, path)
}

fn create_parent(path: &Path) -> Result<(), AnalysisError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| AnalysisError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

fn part_path(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "{}.part",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("tmp")
    ))
}

fn replace_with_part(part: &Path, destination: &Path) -> Result<(), AnalysisError> {
    if destination.exists() {
        fs::remove_file(destination).map_err(|source| AnalysisError::Io {
            path: destination.to_path_buf(),
            source,
        })?;
    }
    fs::rename(part, destination).map_err(|source| AnalysisError::Io {
        path: destination.to_path_buf(),
        source,
    })
}

fn now_rfc3339() -> Result<String, AnalysisError> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

#[cfg(test)]
mod tests {
    use dbn_es_core::{BookLevel, TopOfBook};

    use super::{component_matches, discrepancy_class};

    fn top(bid_price: i64, bid_size: u64, ask_price: i64, ask_size: u64) -> TopOfBook {
        TopOfBook {
            bid: Some(BookLevel {
                price: bid_price,
                size: bid_size,
                order_count: 1,
            }),
            ask: Some(BookLevel {
                price: ask_price,
                size: ask_size,
                order_count: 1,
            }),
        }
    }

    #[test]
    fn classifies_price_and_size_discrepancies() {
        let exchange = top(100, 5, 110, 6);
        assert_eq!(component_matches(exchange, exchange), (true, true));
        assert_eq!(
            discrepancy_class(top(100, 4, 110, 6), exchange, 5),
            "size_only"
        );
        assert_eq!(
            discrepancy_class(top(95, 5, 110, 6), exchange, 5),
            "price_1_tick"
        );
    }
}
