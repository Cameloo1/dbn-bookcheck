//! Core DBN decoding, order-book reconstruction, and liquidity-sweep analysis.
//!
//! The typed streams borrow each record directly from the official DBN decoder's
//! aligned internal buffer. This avoids a record allocation or copy. A record cannot
//! outlive the stream or a call that advances it; callers that need ownership must
//! explicitly copy the record. Zstd input is decompressed into that internal buffer,
//! so compressed bytes necessarily incur decompression and a buffer write even though
//! the DBN record view itself remains zero-copy.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod decode;
mod order_book;
mod sweep;

pub use decode::{
    DecodeError, DecodeStats, SequenceDiscontinuity, SequenceStats, TypedStream, decode_stats,
    stream_mbo, stream_mbp10, stream_ohlcv_1m, stream_trades,
};
pub use order_book::{BookError, BookLevel, BookSet, BookUpdate, InstrumentKey, TopOfBook};
pub use sweep::{SweepConfig, SweepDetector, SweepDirection, SweepError, SweepEvent};
