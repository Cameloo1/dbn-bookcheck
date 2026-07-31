use std::collections::{HashMap, VecDeque};

use dbn::{Action, MboMsg};
use serde::{Deserialize, Serialize};

use crate::{BookSet, InstrumentKey};

/// Four-parameter liquidity-sweep detector configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct SweepConfig {
    /// Number of preceding trades used to define rolling swing extremes.
    pub lookback_trades: usize,
    /// Minimum penetration beyond an extreme.
    pub threshold_ticks: u32,
    /// Maximum time allowed to trade back through the swept level.
    pub reversion_window_ms: u64,
    /// Instrument tick size in DBN fixed-price units.
    pub tick_size: i64,
}

impl SweepConfig {
    /// Validates all parameters before processing data.
    ///
    /// # Errors
    /// Returns [`SweepError::InvalidConfig`] when any parameter is zero, the lookback
    /// is less than two, or the threshold multiplication overflows.
    pub fn validate(self) -> Result<Self, SweepError> {
        if self.lookback_trades < 2 {
            return Err(SweepError::InvalidConfig(
                "lookback_trades must be at least 2".to_owned(),
            ));
        }
        if self.threshold_ticks == 0 {
            return Err(SweepError::InvalidConfig(
                "threshold_ticks must be positive".to_owned(),
            ));
        }
        if self.reversion_window_ms == 0 {
            return Err(SweepError::InvalidConfig(
                "reversion_window_ms must be positive".to_owned(),
            ));
        }
        if self.tick_size <= 0 {
            return Err(SweepError::InvalidConfig(
                "tick_size must be positive".to_owned(),
            ));
        }
        self.tick_size
            .checked_mul(i64::from(self.threshold_ticks))
            .ok_or_else(|| SweepError::InvalidConfig("tick threshold overflows i64".to_owned()))?;
        self.reversion_window_ms
            .checked_mul(1_000_000)
            .ok_or_else(|| {
                SweepError::InvalidConfig("reversion window overflows u64".to_owned())
            })?;
        Ok(self)
    }

    fn threshold_price(self) -> i64 {
        self.tick_size * i64::from(self.threshold_ticks)
    }

    fn window_ns(self) -> u64 {
        self.reversion_window_ms * 1_000_000
    }
}

/// Sweep direction relative to a prior rolling extreme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepDirection {
    /// Price traded above a prior swing high before reverting.
    AboveHigh,
    /// Price traded below a prior swing low before reverting.
    BelowLow,
}

/// One completed liquidity sweep and reversion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SweepEvent {
    /// Publisher-qualified instrument.
    pub instrument: InstrumentKey,
    /// Emission/reversion timestamp; events are emitted monotonically by this field.
    pub timestamp_ns: u64,
    /// Timestamp when price first crossed the configured threshold.
    pub sweep_timestamp_ns: u64,
    /// Timestamp when price reverted through the swept level.
    pub reversion_timestamp_ns: u64,
    /// Direction of the excursion.
    pub direction: SweepDirection,
    /// Prior rolling extreme that was swept, in DBN fixed-price units.
    pub swept_level: i64,
    /// Maximum observed displacement beyond the swept level.
    pub displacement_ticks: u64,
    /// Elapsed nanoseconds from threshold crossing through reversion.
    pub duration_ns: u64,
    /// Visible size at the swept level consumed by the triggering trade, bounded by
    /// both pre-event resting size and trade size.
    pub resting_size_consumed: u64,
}

/// Sweep detector configuration or input failures.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SweepError {
    /// Invalid detector parameters.
    #[error("invalid sweep configuration: {0}")]
    InvalidConfig(String),
    /// A trade record carried an invalid action.
    #[error("invalid action on instrument {instrument_id} at {timestamp_ns}")]
    InvalidAction {
        /// Instrument containing the record.
        instrument_id: u32,
        /// Timestamp of the invalid record.
        timestamp_ns: u64,
    },
    /// Per-instrument trade timestamps regressed.
    #[error(
        "out-of-order trade on instrument {instrument_id}: {timestamp_ns} follows {previous_timestamp_ns}"
    )]
    OutOfOrder {
        /// Instrument containing the record.
        instrument_id: u32,
        /// Regressing timestamp.
        timestamp_ns: u64,
        /// Last accepted timestamp.
        previous_timestamp_ns: u64,
    },
}

#[derive(Debug, Clone, Copy)]
struct TradePoint {
    price: i64,
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    direction: SweepDirection,
    level: i64,
    started_ns: u64,
    max_displacement_ticks: u64,
    resting_size_consumed: u64,
}

#[derive(Default)]
struct InstrumentState {
    trades: VecDeque<TradePoint>,
    pending: Vec<Candidate>,
    last_timestamp_ns: Option<u64>,
}

/// Stateful detector over MBO trade records and the reconstructed pre-event book.
pub struct SweepDetector {
    config: SweepConfig,
    instruments: HashMap<InstrumentKey, InstrumentState>,
}

impl SweepDetector {
    /// Creates a detector after validating its four configured parameters.
    ///
    /// # Errors
    /// Returns [`SweepError::InvalidConfig`] for invalid parameters.
    pub fn new(config: SweepConfig) -> Result<Self, SweepError> {
        Ok(Self {
            config: config.validate()?,
            instruments: HashMap::new(),
        })
    }

    /// Observes an MBO record and emits zero or more completed sweep events.
    ///
    /// Non-trade records and records received before the book has a complete baseline
    /// are ignored. Call this before applying the same MBO record to `books` so the
    /// visible-size measurement reflects pre-event resting state.
    ///
    /// # Errors
    /// Returns an error for an invalid action byte or regressing per-instrument trade
    /// timestamp.
    pub fn observe(
        &mut self,
        message: &MboMsg,
        books: &BookSet,
    ) -> Result<Vec<SweepEvent>, SweepError> {
        let action = message.action().map_err(|_| SweepError::InvalidAction {
            instrument_id: message.hd.instrument_id,
            timestamp_ns: message.hd.ts_event,
        })?;
        if action != Action::Trade {
            return Ok(Vec::new());
        }

        let instrument = InstrumentKey::from(message);
        if books.top(instrument).is_none() {
            return Ok(Vec::new());
        }
        let state = self.instruments.entry(instrument).or_default();
        if let Some(previous) = state.last_timestamp_ns
            && message.hd.ts_event < previous
        {
            return Err(SweepError::OutOfOrder {
                instrument_id: instrument.instrument_id,
                timestamp_ns: message.hd.ts_event,
                previous_timestamp_ns: previous,
            });
        }
        state.last_timestamp_ns = Some(message.hd.ts_event);

        let mut events = complete_or_expire_candidates(
            self.config,
            instrument,
            state,
            message.hd.ts_event,
            message.price,
        );
        start_candidate_if_crossed(self.config, instrument, state, message, books);
        state.trades.push_back(TradePoint {
            price: message.price,
        });
        while state.trades.len() > self.config.lookback_trades {
            state.trades.pop_front();
        }
        events.sort_by_key(|event| event.timestamp_ns);
        Ok(events)
    }
}

fn complete_or_expire_candidates(
    config: SweepConfig,
    instrument: InstrumentKey,
    state: &mut InstrumentState,
    timestamp_ns: u64,
    price: i64,
) -> Vec<SweepEvent> {
    let mut events = Vec::new();
    state.pending.retain_mut(|candidate| {
        let age = timestamp_ns.saturating_sub(candidate.started_ns);
        if age > config.window_ns() {
            return false;
        }
        let displacement = match candidate.direction {
            SweepDirection::AboveHigh if price > candidate.level => {
                price_distance_ticks(price, candidate.level, config.tick_size)
            }
            SweepDirection::BelowLow if price < candidate.level => {
                price_distance_ticks(candidate.level, price, config.tick_size)
            }
            _ => 0,
        };
        candidate.max_displacement_ticks = candidate.max_displacement_ticks.max(displacement);
        let reverted = match candidate.direction {
            SweepDirection::AboveHigh => price <= candidate.level,
            SweepDirection::BelowLow => price >= candidate.level,
        };
        if reverted {
            events.push(SweepEvent {
                instrument,
                timestamp_ns,
                sweep_timestamp_ns: candidate.started_ns,
                reversion_timestamp_ns: timestamp_ns,
                direction: candidate.direction,
                swept_level: candidate.level,
                displacement_ticks: candidate.max_displacement_ticks,
                duration_ns: age,
                resting_size_consumed: candidate.resting_size_consumed,
            });
            false
        } else {
            true
        }
    });
    events
}

fn start_candidate_if_crossed(
    config: SweepConfig,
    instrument: InstrumentKey,
    state: &mut InstrumentState,
    message: &MboMsg,
    books: &BookSet,
) {
    if state.trades.len() < config.lookback_trades {
        return;
    }
    let high = state.trades.iter().map(|trade| trade.price).max();
    let low = state.trades.iter().map(|trade| trade.price).min();
    let threshold = config.threshold_price();
    if let Some(level) = high
        && message.price >= level.saturating_add(threshold)
        && !state
            .pending
            .iter()
            .any(|candidate| candidate.direction == SweepDirection::AboveHigh)
    {
        state.pending.push(Candidate {
            direction: SweepDirection::AboveHigh,
            level,
            started_ns: message.hd.ts_event,
            max_displacement_ticks: price_distance_ticks(message.price, level, config.tick_size),
            resting_size_consumed: books
                .level_size(instrument, false, level)
                .unwrap_or(0)
                .min(u64::from(message.size)),
        });
    }
    if let Some(level) = low
        && message.price <= level.saturating_sub(threshold)
        && !state
            .pending
            .iter()
            .any(|candidate| candidate.direction == SweepDirection::BelowLow)
    {
        state.pending.push(Candidate {
            direction: SweepDirection::BelowLow,
            level,
            started_ns: message.hd.ts_event,
            max_displacement_ticks: price_distance_ticks(level, message.price, config.tick_size),
            resting_size_consumed: books
                .level_size(instrument, true, level)
                .unwrap_or(0)
                .min(u64::from(message.size)),
        });
    }
}

fn price_distance_ticks(high: i64, low: i64, tick_size: i64) -> u64 {
    let distance = i128::from(high) - i128::from(low);
    u64::try_from(distance / i128::from(tick_size)).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use dbn::{Action, FlagSet, MboMsg, Side};
    use proptest::prelude::*;

    use crate::BookSet;

    use super::{SweepConfig, SweepDetector, SweepDirection};

    const TICK: i64 = 250_000_000;
    const BASE: i64 = 5_000_000_000_000;

    fn message(timestamp: u64, action: Action, side: Side, price: i64, quantity: u32) -> MboMsg {
        let mut message = MboMsg::default();
        message.hd.publisher_id = 1;
        message.hd.instrument_id = 2;
        message.hd.ts_event = timestamp;
        message.order_id = timestamp;
        message.action = i8::try_from(u8::from(action)).expect("DBN actions fit in i8");
        message.side = i8::try_from(u8::from(side)).expect("DBN sides fit in i8");
        message.price = price;
        message.size = quantity;
        message.flags = FlagSet::empty().set_last();
        message
    }

    #[test]
    fn detects_high_sweep_and_reversion() {
        let mut books = BookSet::default();
        books
            .apply(&message(1, Action::Clear, Side::None, 0, 0))
            .expect("baseline");
        let mut detector = SweepDetector::new(SweepConfig {
            lookback_trades: 3,
            threshold_ticks: 2,
            reversion_window_ms: 1,
            tick_size: TICK,
        })
        .expect("config");
        for (timestamp, price) in [(2, BASE), (3, BASE + TICK), (4, BASE)] {
            assert!(
                detector
                    .observe(
                        &message(timestamp, Action::Trade, Side::Bid, price, 1),
                        &books
                    )
                    .expect("trade")
                    .is_empty()
            );
        }
        assert!(
            detector
                .observe(
                    &message(5, Action::Trade, Side::Bid, BASE + 3 * TICK, 1),
                    &books
                )
                .expect("cross")
                .is_empty()
        );
        let events = detector
            .observe(
                &message(6, Action::Trade, Side::Ask, BASE + TICK, 1),
                &books,
            )
            .expect("reversion");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].direction, SweepDirection::AboveHigh);
        assert_eq!(events[0].displacement_ticks, 2);
    }

    #[test]
    fn rejects_invalid_configuration() {
        assert!(
            SweepDetector::new(SweepConfig {
                lookback_trades: 1,
                threshold_ticks: 0,
                reversion_window_ms: 0,
                tick_size: 0,
            })
            .is_err()
        );
    }

    proptest! {
        #[test]
        fn arbitrary_records_never_panic_and_events_preserve_invariants(
            inputs in prop::collection::vec((any::<i64>(), any::<u32>(), any::<i8>()), 0..160)
        ) {
            let mut books = BookSet::default();
            books
                .apply(&message(1, Action::Clear, Side::None, 0, 0))
                .expect("baseline");
            let mut detector = SweepDetector::new(SweepConfig {
                lookback_trades: 2,
                threshold_ticks: 1,
                reversion_window_ms: 10,
                tick_size: 1,
            })
            .expect("valid config");
            let mut observed_prices = Vec::new();
            let mut previous_event_timestamp = None;

            for (index, (price, size, raw_action)) in inputs.into_iter().enumerate() {
                let mut record = MboMsg::default();
                record.hd.publisher_id = 1;
                record.hd.instrument_id = 2;
                record.hd.ts_event = u64::try_from(index).expect("bounded index") + 2;
                record.action = raw_action;
                record.side = i8::try_from(u8::from(Side::Bid)).expect("side fits i8");
                record.price = price;
                record.size = size;
                record.flags = FlagSet::empty().set_last();

                let result = detector.observe(&record, &books);
                if raw_action == i8::try_from(u8::from(Action::Trade)).expect("action fits i8") {
                    observed_prices.push(price);
                }
                if let Ok(events) = result {
                    for event in events {
                        if let Some(previous) = previous_event_timestamp {
                            prop_assert!(event.timestamp_ns >= previous);
                        }
                        previous_event_timestamp = Some(event.timestamp_ns);
                        let minimum = observed_prices.iter().copied().min().expect("event requires history");
                        let maximum = observed_prices.iter().copied().max().expect("event requires history");
                        prop_assert!(event.swept_level >= minimum);
                        prop_assert!(event.swept_level <= maximum);
                    }
                }
            }
        }
    }
}
