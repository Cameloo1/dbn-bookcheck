use std::{
    fs::{self, File},
    io::{self, Write},
    num::NonZeroU64,
    path::{Path, PathBuf},
};

use dbn::{
    Action, FlagSet, MboMsg, Mbp10Msg, Metadata, OhlcvMsg, RType, RecordHeader, SType, Schema,
    Side, TradeMsg,
    encode::{DbnEncodable, DbnEncoder, EncodeRecord},
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::acquisition::{Manifest, ManifestEntry};

const DATASET: &str = "GLBX.MDP3";
const SYMBOL: &str = "ES.SYNTHETIC";
const START: &str = "2025-01-02T14:30:00Z";
const END: &str = "2025-01-02T14:31:00Z";
const START_NS: u64 = 1_735_828_200_000_000_000;
const PUBLISHER_ID: u16 = 1;
const INSTRUMENT_ID: u32 = 4_916;
const BASE_PRICE: i64 = 5_000_000_000_000;
const TICK_SIZE: i64 = 250_000_000;

/// Errors returned by deterministic sample generation.
#[derive(Debug, thiserror::Error)]
pub enum SampleError {
    /// A filesystem operation failed.
    #[error("I/O error for {path}: {source}")]
    Io {
        /// File or directory being accessed.
        path: PathBuf,
        /// Original I/O error.
        #[source]
        source: io::Error,
    },
    /// The official DBN encoder rejected metadata or a record.
    #[error("DBN encode failed: {0}")]
    Dbn(#[from] dbn::Error),
    /// Manifest serialization failed.
    #[error("manifest serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

/// Generates four small, deterministic, Zstd-compressed DBN files and a manifest.
///
/// The records are intentionally synthetic and are only suitable for clean-checkout
/// verification, tests, and demonstrations. Re-running the generator produces the
/// same DBN and manifest bytes.
///
/// # Errors
/// Returns an error if the output directory cannot be created, DBN encoding fails,
/// compression fails, or a generated file cannot be written.
pub fn generate_sample(output_dir: &Path) -> Result<Manifest, SampleError> {
    fs::create_dir_all(output_dir).map_err(|source| io_error(output_dir, source))?;

    let (mbo, mbp10, trades) = market_records();
    let ohlcv = ohlcv_records();
    let specs = [
        encode_file(output_dir, "sample-mbo.dbn.zst", Schema::Mbo, &mbo)?,
        encode_file(output_dir, "sample-mbp-10.dbn.zst", Schema::Mbp10, &mbp10)?,
        encode_file(output_dir, "sample-trades.dbn.zst", Schema::Trades, &trades)?,
        encode_file(
            output_dir,
            "sample-ohlcv-1m.dbn.zst",
            Schema::Ohlcv1M,
            &ohlcv,
        )?,
    ];

    let manifest = Manifest {
        version: 1,
        generated_at: END.to_owned(),
        source: "synthetic".to_owned(),
        session_reason: "Deterministic synthetic clean-checkout fixture; not market data."
            .to_owned(),
        entries: specs
            .into_iter()
            .map(|spec| ManifestEntry {
                path: spec.path.to_string_lossy().replace('\\', "/"),
                schema: spec.schema.as_str().to_owned(),
                source: "synthetic".to_owned(),
                dataset: DATASET.to_owned(),
                symbol: SYMBOL.to_owned(),
                start: START.to_owned(),
                end: END.to_owned(),
                compressed_bytes: spec.compressed_bytes,
                uncompressed_bytes: spec.uncompressed_bytes,
                record_count: spec.record_count,
                sha256: spec.sha256,
                acquired_at: END.to_owned(),
            })
            .collect(),
    };
    write_json_atomic(&output_dir.join("manifest.json"), &manifest)?;
    Ok(manifest)
}

struct EncodedFile {
    path: PathBuf,
    schema: Schema,
    compressed_bytes: u64,
    uncompressed_bytes: u64,
    record_count: u64,
    sha256: String,
}

fn encode_file<T: DbnEncodable>(
    output_dir: &Path,
    name: &str,
    schema: Schema,
    records: &[T],
) -> Result<EncodedFile, SampleError> {
    let metadata = Metadata::builder()
        .dataset(DATASET)
        .schema(Some(schema))
        .start(START_NS)
        .end(NonZeroU64::new(START_NS + 60_000_000_000))
        .stype_in(Some(SType::InstrumentId))
        .stype_out(SType::InstrumentId)
        .symbols(vec![SYMBOL.to_owned()])
        .build();
    let mut raw = Vec::new();
    {
        let mut encoder = DbnEncoder::new(&mut raw, &metadata)?;
        encoder.encode_records(records)?;
        encoder.flush()?;
    }
    let compressed = zstd::stream::encode_all(raw.as_slice(), 3)
        .map_err(|source| io_error(output_dir.join(name), source))?;
    let path = output_dir.join(name);
    write_bytes_atomic(&path, &compressed)?;

    Ok(EncodedFile {
        path,
        schema,
        compressed_bytes: compressed.len() as u64,
        uncompressed_bytes: raw.len() as u64,
        record_count: records.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&compressed)),
    })
}

fn market_records() -> (Vec<MboMsg>, Vec<Mbp10Msg>, Vec<TradeMsg>) {
    let mut mbo = Vec::with_capacity(55);
    let mut mbp10 = Vec::with_capacity(54);
    let mut trades = Vec::with_capacity(52);

    mbo.push(mbo_message(1, 0, Action::Clear, Side::None, 0, 0));
    let bid = mbo_message(2, 1, Action::Add, Side::Bid, BASE_PRICE, 10);
    mbo.push(bid.clone());
    mbp10.push(mbp_message(&bid, true, false));
    let ask = mbo_message(3, 2, Action::Add, Side::Ask, BASE_PRICE + TICK_SIZE, 10);
    mbo.push(ask.clone());
    mbp10.push(mbp_message(&ask, true, true));

    for index in 0..50_u32 {
        let sequence = index + 4;
        let price = if index % 2 == 0 {
            BASE_PRICE
        } else {
            BASE_PRICE + TICK_SIZE
        };
        push_trade(sequence, price, &mut mbo, &mut mbp10, &mut trades);
    }
    push_trade(
        54,
        BASE_PRICE + 5 * TICK_SIZE,
        &mut mbo,
        &mut mbp10,
        &mut trades,
    );
    push_trade(
        55,
        BASE_PRICE + TICK_SIZE,
        &mut mbo,
        &mut mbp10,
        &mut trades,
    );
    (mbo, mbp10, trades)
}

fn push_trade(
    sequence: u32,
    price: i64,
    mbo: &mut Vec<MboMsg>,
    mbp10: &mut Vec<Mbp10Msg>,
    trades: &mut Vec<TradeMsg>,
) {
    let message = mbo_message(sequence, 0, Action::Trade, Side::Bid, price, 1);
    mbp10.push(mbp_message(&message, true, true));
    trades.push(trade_message(&message));
    mbo.push(message);
}

fn header<T: dbn::HasRType>(rtype: RType, sequence: u32) -> RecordHeader {
    RecordHeader::new::<T>(
        rtype as u8,
        PUBLISHER_ID,
        INSTRUMENT_ID,
        START_NS + u64::from(sequence) * 100_000_000,
    )
}

fn mbo_message(
    sequence: u32,
    order_id: u64,
    action: Action,
    side: Side,
    price: i64,
    quantity: u32,
) -> MboMsg {
    MboMsg {
        hd: header::<MboMsg>(RType::Mbo, sequence),
        order_id,
        price,
        size: quantity,
        flags: FlagSet::empty().set_last(),
        channel_id: 0,
        action: action as std::ffi::c_char,
        side: side as std::ffi::c_char,
        ts_recv: START_NS + u64::from(sequence) * 100_000_000 + 1_000,
        ts_in_delta: 1_000,
        sequence,
    }
}

fn mbp_message(mbo: &MboMsg, has_bid: bool, has_ask: bool) -> Mbp10Msg {
    let mut message = Mbp10Msg {
        hd: header::<Mbp10Msg>(RType::Mbp10, mbo.sequence),
        price: mbo.price,
        size: mbo.size,
        action: mbo.action,
        side: mbo.side,
        flags: mbo.flags,
        depth: 0,
        ts_recv: mbo.ts_recv,
        ts_in_delta: mbo.ts_in_delta,
        sequence: mbo.sequence,
        ..Mbp10Msg::default()
    };
    if has_bid {
        message.levels[0].bid_px = BASE_PRICE;
        message.levels[0].bid_sz = 10;
        message.levels[0].bid_ct = 1;
    }
    if has_ask {
        message.levels[0].ask_px = BASE_PRICE + TICK_SIZE;
        message.levels[0].ask_sz = 10;
        message.levels[0].ask_ct = 1;
    }
    message
}

fn trade_message(mbo: &MboMsg) -> TradeMsg {
    TradeMsg {
        hd: header::<TradeMsg>(RType::Mbp0, mbo.sequence),
        price: mbo.price,
        size: mbo.size,
        action: mbo.action,
        side: mbo.side,
        flags: mbo.flags,
        depth: 0,
        ts_recv: mbo.ts_recv,
        ts_in_delta: mbo.ts_in_delta,
        sequence: mbo.sequence,
    }
}

fn ohlcv_records() -> Vec<OhlcvMsg> {
    let mut message = OhlcvMsg::default_for_schema(Schema::Ohlcv1M);
    message.hd.publisher_id = PUBLISHER_ID;
    message.hd.instrument_id = INSTRUMENT_ID;
    message.hd.ts_event = START_NS;
    message.open = BASE_PRICE;
    message.high = BASE_PRICE + 5 * TICK_SIZE;
    message.low = BASE_PRICE - TICK_SIZE;
    message.close = BASE_PRICE + TICK_SIZE;
    message.volume = 52;
    vec![message]
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), SampleError> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_bytes_atomic(path, &bytes)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), SampleError> {
    let temp = path.with_extension("tmp");
    let mut file = File::create(&temp).map_err(|source| io_error(&temp, source))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| io_error(&temp, source))?;
    drop(file);
    if path.exists() {
        fs::remove_file(path).map_err(|source| io_error(path, source))?;
    }
    fs::rename(&temp, path).map_err(|source| io_error(path, source))
}

fn io_error(path: impl AsRef<Path>, source: io::Error) -> SampleError {
    SampleError::Io {
        path: path.as_ref().to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::generate_sample;

    #[test]
    fn generation_is_byte_deterministic() {
        let root = std::env::temp_dir().join(format!("dbn-es-sample-{}", std::process::id()));
        let first = root.join("first");
        let second = root.join("second");
        let first_manifest = generate_sample(&first).expect("first generation");
        let first_manifest_bytes = fs::read(first.join("manifest.json")).expect("first manifest");
        generate_sample(&first).expect("repeat first generation");
        assert_eq!(
            fs::read(first.join("manifest.json")).expect("repeated manifest"),
            first_manifest_bytes
        );
        let second_manifest = generate_sample(&second).expect("second generation");

        assert_eq!(first_manifest.entries.len(), 4);
        for (left, right) in first_manifest.entries.iter().zip(&second_manifest.entries) {
            assert_eq!(left.schema, right.schema);
            assert_eq!(left.sha256, right.sha256);
            assert_eq!(left.record_count, right.record_count);
        }
        fs::remove_dir_all(root).expect("remove generated test fixture");
    }
}
