use std::collections::{BTreeMap, HashMap};

use dbn::{Action, MboMsg, Side, UNDEF_PRICE};
use serde::Serialize;

/// Publisher-qualified instrument identity used to partition independent books.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct InstrumentKey {
    /// DBN publisher ID.
    pub publisher_id: u16,
    /// DBN numeric instrument ID.
    pub instrument_id: u32,
}

impl From<&MboMsg> for InstrumentKey {
    fn from(message: &MboMsg) -> Self {
        Self {
            publisher_id: message.hd.publisher_id,
            instrument_id: message.hd.instrument_id,
        }
    }
}

/// One aggregated price level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BookLevel {
    /// Fixed-point DBN price in 1e-9 units.
    pub price: i64,
    /// Aggregate displayed quantity.
    pub size: u64,
    /// Number of individual resting orders.
    pub order_count: u32,
}

/// Best displayed bid and ask for one instrument.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TopOfBook {
    /// Highest bid, when present.
    pub bid: Option<BookLevel>,
    /// Lowest ask, when present.
    pub ask: Option<BookLevel>,
}

/// Result of applying one MBO record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BookUpdate {
    /// Book partition affected by the record.
    pub instrument: InstrumentKey,
    /// Event timestamp.
    pub timestamp_ns: u64,
    /// Venue sequence from the MBO record.
    pub sequence: u32,
    /// Whether this record ended the normalized event.
    pub event_complete: bool,
    /// Whether the record changed resting order state.
    pub state_changed: bool,
    /// Whether the book has a complete baseline and can be inspected.
    pub book_valid: bool,
    /// Current top of book when valid, including intermediate multi-record states.
    pub top: Option<TopOfBook>,
    /// Why an update was withheld while the book was invalid.
    pub withheld_reason: Option<&'static str>,
}

/// Explicit failures while applying MBO order state.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BookError {
    /// DBN action or side byte was invalid.
    #[error("invalid DBN enum on {instrument_id} at {timestamp_ns}: {field}")]
    InvalidEnum {
        /// Instrument containing the invalid value.
        instrument_id: u32,
        /// Timestamp of the bad record.
        timestamp_ns: u64,
        /// Field that failed conversion.
        field: &'static str,
    },
    /// An add reused a live order identifier.
    #[error("duplicate order {order_id} on {instrument_id} at {timestamp_ns}")]
    DuplicateOrder {
        /// Instrument containing the order.
        instrument_id: u32,
        /// Duplicate venue order ID.
        order_id: u64,
        /// Timestamp of the add.
        timestamp_ns: u64,
    },
    /// A cancel or modify referenced an absent order.
    #[error("unknown order {order_id} for {action} on {instrument_id} at {timestamp_ns}")]
    UnknownOrder {
        /// Instrument containing the event.
        instrument_id: u32,
        /// Missing venue order ID.
        order_id: u64,
        /// Action requesting the order.
        action: &'static str,
        /// Event timestamp.
        timestamp_ns: u64,
    },
    /// A cancel attempted to remove more than the displayed order size.
    #[error(
        "cancel size {cancel_size} exceeds order {order_id} remaining size {remaining_size} on {instrument_id} at {timestamp_ns}"
    )]
    OverCancel {
        /// Instrument containing the order.
        instrument_id: u32,
        /// Venue order ID.
        order_id: u64,
        /// Requested removal.
        cancel_size: u32,
        /// Quantity available before the cancel.
        remaining_size: u32,
        /// Event timestamp.
        timestamp_ns: u64,
    },
    /// A state-changing record had no bid or ask side.
    #[error("action {action} has no book side on {instrument_id} at {timestamp_ns}")]
    MissingSide {
        /// Instrument containing the event.
        instrument_id: u32,
        /// Action requiring the side.
        action: &'static str,
        /// Event timestamp.
        timestamp_ns: u64,
    },
    /// A valid book received a record older than its preceding record.
    #[error(
        "out-of-order event on {instrument_id}: timestamp {timestamp_ns} follows {previous_timestamp_ns}"
    )]
    OutOfOrder {
        /// Instrument containing the event.
        instrument_id: u32,
        /// Regressing timestamp.
        timestamp_ns: u64,
        /// Last accepted timestamp.
        previous_timestamp_ns: u64,
    },
    /// Aggregate quantity or count overflowed its durable representation.
    #[error("book aggregate overflow on {instrument_id} at price {price}")]
    AggregateOverflow {
        /// Instrument containing the level.
        instrument_id: u32,
        /// Fixed-point level price.
        price: i64,
    },
    /// Internal order and price-level state diverged.
    #[error("book invariant failed on {instrument_id} at price {price}")]
    Invariant {
        /// Instrument containing the inconsistent level.
        instrument_id: u32,
        /// Fixed-point level price.
        price: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BookSide {
    Bid,
    Ask,
}

impl TryFrom<Side> for BookSide {
    type Error = ();

    fn try_from(value: Side) -> Result<Self, Self::Error> {
        match value {
            Side::Bid => Ok(Self::Bid),
            Side::Ask => Ok(Self::Ask),
            Side::None => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Order {
    side: BookSide,
    price: i64,
    size: u32,
    counted: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct Aggregate {
    size: u64,
    order_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Health {
    AwaitingBaseline,
    Rebuilding,
    Valid,
}

struct Book {
    health: Health,
    orders: HashMap<u64, Order>,
    bids: BTreeMap<i64, Aggregate>,
    asks: BTreeMap<i64, Aggregate>,
    last_timestamp_ns: Option<u64>,
}

impl Default for Book {
    fn default() -> Self {
        Self {
            health: Health::AwaitingBaseline,
            orders: HashMap::new(),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            last_timestamp_ns: None,
        }
    }
}

/// Multi-instrument MBO book reconstructor with snapshot recovery boundaries.
#[derive(Default)]
pub struct BookSet {
    books: HashMap<InstrumentKey, Book>,
}

impl BookSet {
    /// Applies one MBO record and returns the resulting observable state.
    ///
    /// Books begin invalid because a mid-session file has no baseline. A clear event
    /// starts recovery, and the book becomes inspectable only at `F_LAST`. A
    /// `F_MAYBE_BAD_BOOK` record invalidates and clears state until another clear.
    ///
    /// # Errors
    /// Returns a [`BookError`] for invalid enum bytes, out-of-order input, unknown or
    /// duplicate orders, excessive cancels, arithmetic overflow, or broken internal
    /// level invariants.
    pub fn apply(&mut self, message: &MboMsg) -> Result<BookUpdate, BookError> {
        let key = InstrumentKey::from(message);
        let book = self.books.entry(key).or_default();
        let action = message.action().map_err(|_| BookError::InvalidEnum {
            instrument_id: key.instrument_id,
            timestamp_ns: message.hd.ts_event,
            field: "action",
        })?;

        if message.flags.is_maybe_bad_book() {
            book.clear();
            book.health = Health::AwaitingBaseline;
            return Ok(make_update(
                key,
                message,
                false,
                book,
                Some("F_MAYBE_BAD_BOOK requires a new clear/snapshot baseline"),
            ));
        }

        if action == Action::Clear {
            book.clear();
            book.health = if message.flags.is_last() {
                Health::Valid
            } else {
                Health::Rebuilding
            };
            book.last_timestamp_ns = Some(message.hd.ts_event);
            return Ok(make_update(key, message, true, book, None));
        }

        if book.health == Health::AwaitingBaseline {
            return Ok(make_update(
                key,
                message,
                false,
                book,
                Some("no complete clear/snapshot baseline has been observed"),
            ));
        }

        if book.health == Health::Valid
            && let Some(previous) = book.last_timestamp_ns
            && message.hd.ts_event < previous
        {
            return Err(BookError::OutOfOrder {
                instrument_id: key.instrument_id,
                timestamp_ns: message.hd.ts_event,
                previous_timestamp_ns: previous,
            });
        }

        let changed = match action {
            Action::Add => {
                let side = parse_side(message, "add")?;
                if message.flags.is_tob() {
                    book.remove_side(side, key.instrument_id)?;
                    if message.price == UNDEF_PRICE {
                        true
                    } else {
                        book.add(message, side, false, key.instrument_id)?;
                        true
                    }
                } else {
                    book.add(message, side, true, key.instrument_id)?;
                    true
                }
            }
            Action::Cancel => {
                book.cancel(message, key.instrument_id)?;
                true
            }
            Action::Modify => {
                let side = parse_side(message, "modify")?;
                book.modify(message, side, key.instrument_id)?;
                true
            }
            Action::Trade | Action::Fill | Action::None => false,
            Action::Clear => unreachable!("clear returned before action dispatch"),
        };

        if book.health == Health::Rebuilding && message.flags.is_last() {
            book.health = Health::Valid;
        }
        book.last_timestamp_ns = Some(message.hd.ts_event);
        Ok(make_update(key, message, changed, book, None))
    }

    /// Returns a valid current top of book for an instrument.
    #[must_use]
    pub fn top(&self, instrument: InstrumentKey) -> Option<TopOfBook> {
        self.books
            .get(&instrument)
            .filter(|book| book.health == Health::Valid)
            .map(Book::top)
    }

    /// Returns aggregate displayed size at a price on the specified side.
    #[must_use]
    pub fn level_size(&self, instrument: InstrumentKey, bid_side: bool, price: i64) -> Option<u64> {
        self.books
            .get(&instrument)
            .filter(|book| book.health == Health::Valid)
            .and_then(|book| {
                let levels = if bid_side { &book.bids } else { &book.asks };
                levels.get(&price).map(|level| level.size)
            })
    }
}

fn parse_side(message: &MboMsg, action: &'static str) -> Result<BookSide, BookError> {
    let side = message.side().map_err(|_| BookError::InvalidEnum {
        instrument_id: message.hd.instrument_id,
        timestamp_ns: message.hd.ts_event,
        field: "side",
    })?;
    BookSide::try_from(side).map_err(|()| BookError::MissingSide {
        instrument_id: message.hd.instrument_id,
        action,
        timestamp_ns: message.hd.ts_event,
    })
}

fn make_update(
    key: InstrumentKey,
    message: &MboMsg,
    state_changed: bool,
    book: &Book,
    withheld_reason: Option<&'static str>,
) -> BookUpdate {
    let book_valid = book.health == Health::Valid;
    BookUpdate {
        instrument: key,
        timestamp_ns: message.hd.ts_event,
        sequence: message.sequence,
        event_complete: message.flags.is_last(),
        state_changed,
        book_valid,
        top: book_valid.then(|| book.top()),
        withheld_reason,
    }
}

impl Book {
    fn clear(&mut self) {
        self.orders.clear();
        self.bids.clear();
        self.asks.clear();
        self.last_timestamp_ns = None;
    }

    fn levels_mut(&mut self, side: BookSide) -> &mut BTreeMap<i64, Aggregate> {
        match side {
            BookSide::Bid => &mut self.bids,
            BookSide::Ask => &mut self.asks,
        }
    }

    fn add(
        &mut self,
        message: &MboMsg,
        side: BookSide,
        counted: bool,
        instrument_id: u32,
    ) -> Result<(), BookError> {
        if self.orders.contains_key(&message.order_id) {
            return Err(BookError::DuplicateOrder {
                instrument_id,
                order_id: message.order_id,
                timestamp_ns: message.hd.ts_event,
            });
        }
        self.add_level(side, message.price, message.size, counted, instrument_id)?;
        self.orders.insert(
            message.order_id,
            Order {
                side,
                price: message.price,
                size: message.size,
                counted,
            },
        );
        Ok(())
    }

    fn cancel(&mut self, message: &MboMsg, instrument_id: u32) -> Result<(), BookError> {
        let order = self
            .orders
            .get(&message.order_id)
            .copied()
            .ok_or(BookError::UnknownOrder {
                instrument_id,
                order_id: message.order_id,
                action: "cancel",
                timestamp_ns: message.hd.ts_event,
            })?;
        if message.size > order.size {
            return Err(BookError::OverCancel {
                instrument_id,
                order_id: message.order_id,
                cancel_size: message.size,
                remaining_size: order.size,
                timestamp_ns: message.hd.ts_event,
            });
        }
        let remaining = order.size - message.size;
        self.subtract_level(
            order.side,
            order.price,
            message.size,
            order.counted && remaining == 0,
            instrument_id,
        )?;
        if remaining == 0 {
            self.orders.remove(&message.order_id);
        } else if let Some(existing) = self.orders.get_mut(&message.order_id) {
            existing.size = remaining;
        }
        Ok(())
    }

    fn modify(
        &mut self,
        message: &MboMsg,
        side: BookSide,
        instrument_id: u32,
    ) -> Result<(), BookError> {
        let existing =
            self.orders
                .get(&message.order_id)
                .copied()
                .ok_or(BookError::UnknownOrder {
                    instrument_id,
                    order_id: message.order_id,
                    action: "modify",
                    timestamp_ns: message.hd.ts_event,
                })?;
        self.subtract_level(
            existing.side,
            existing.price,
            existing.size,
            existing.counted,
            instrument_id,
        )?;
        self.add_level(
            side,
            message.price,
            message.size,
            existing.counted,
            instrument_id,
        )?;
        self.orders.insert(
            message.order_id,
            Order {
                side,
                price: message.price,
                size: message.size,
                counted: existing.counted,
            },
        );
        Ok(())
    }

    fn remove_side(&mut self, side: BookSide, instrument_id: u32) -> Result<(), BookError> {
        let removals: Vec<(u64, Order)> = self
            .orders
            .iter()
            .filter(|(_, order)| order.side == side)
            .map(|(id, order)| (*id, *order))
            .collect();
        for (id, order) in removals {
            self.subtract_level(
                order.side,
                order.price,
                order.size,
                order.counted,
                instrument_id,
            )?;
            self.orders.remove(&id);
        }
        Ok(())
    }

    fn add_level(
        &mut self,
        side: BookSide,
        price: i64,
        quantity: u32,
        counted: bool,
        instrument_id: u32,
    ) -> Result<(), BookError> {
        let level = self.levels_mut(side).entry(price).or_default();
        level.size =
            level
                .size
                .checked_add(u64::from(quantity))
                .ok_or(BookError::AggregateOverflow {
                    instrument_id,
                    price,
                })?;
        if counted {
            level.order_count =
                level
                    .order_count
                    .checked_add(1)
                    .ok_or(BookError::AggregateOverflow {
                        instrument_id,
                        price,
                    })?;
        }
        Ok(())
    }

    fn subtract_level(
        &mut self,
        side: BookSide,
        price: i64,
        quantity: u32,
        remove_count: bool,
        instrument_id: u32,
    ) -> Result<(), BookError> {
        let levels = self.levels_mut(side);
        let level = levels.get_mut(&price).ok_or(BookError::Invariant {
            instrument_id,
            price,
        })?;
        level.size = level
            .size
            .checked_sub(u64::from(quantity))
            .ok_or(BookError::Invariant {
                instrument_id,
                price,
            })?;
        if remove_count {
            level.order_count = level
                .order_count
                .checked_sub(1)
                .ok_or(BookError::Invariant {
                    instrument_id,
                    price,
                })?;
        }
        if level.size == 0 {
            if level.order_count != 0 {
                return Err(BookError::Invariant {
                    instrument_id,
                    price,
                });
            }
            levels.remove(&price);
        }
        Ok(())
    }

    fn top(&self) -> TopOfBook {
        TopOfBook {
            bid: self.bids.last_key_value().map(|(price, level)| BookLevel {
                price: *price,
                size: level.size,
                order_count: level.order_count,
            }),
            ask: self.asks.first_key_value().map(|(price, level)| BookLevel {
                price: *price,
                size: level.size,
                order_count: level.order_count,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use dbn::{Action, FlagSet, MboMsg, Side};

    use super::{BookError, BookSet, InstrumentKey, TopOfBook};

    const PRICE: i64 = 5_000_000_000_000;

    fn message(
        timestamp: u64,
        order_id: u64,
        action: Action,
        side: Side,
        price: i64,
        quantity: u32,
    ) -> MboMsg {
        let mut message = MboMsg::default();
        message.hd.publisher_id = 1;
        message.hd.instrument_id = 7;
        message.hd.ts_event = timestamp;
        message.order_id = order_id;
        message.action = i8::try_from(u8::from(action)).expect("DBN actions fit in i8");
        message.side = i8::try_from(u8::from(side)).expect("DBN sides fit in i8");
        message.price = price;
        message.size = quantity;
        message.flags = FlagSet::empty().set_last();
        message
    }

    fn valid_book() -> BookSet {
        let mut books = BookSet::default();
        books
            .apply(&message(1, 0, Action::Clear, Side::None, 0, 0))
            .expect("clear must establish baseline");
        books
    }

    #[test]
    fn withholds_updates_until_clear_baseline() {
        let mut books = BookSet::default();
        let update = books
            .apply(&message(1, 1, Action::Add, Side::Bid, PRICE, 2))
            .expect("pre-baseline add is withheld");
        assert!(!update.book_valid);
        assert!(!update.state_changed);
    }

    #[test]
    fn applies_every_order_action_and_aggregates_levels() {
        let mut books = valid_book();
        let key = InstrumentKey {
            publisher_id: 1,
            instrument_id: 7,
        };
        books
            .apply(&message(2, 1, Action::Add, Side::Bid, PRICE, 4))
            .expect("add");
        books
            .apply(&message(3, 2, Action::Add, Side::Bid, PRICE, 3))
            .expect("second add");
        assert_eq!(books.top(key).expect("valid").bid.expect("bid").size, 7);

        books
            .apply(&message(4, 1, Action::Cancel, Side::Bid, PRICE, 1))
            .expect("partial cancel");
        books
            .apply(&message(5, 2, Action::Modify, Side::Ask, PRICE + 1, 5))
            .expect("modify price and side");
        let top = books.top(key).expect("valid");
        assert_eq!(top.bid.expect("bid").size, 3);
        assert_eq!(top.ask.expect("ask").size, 5);

        for action in [Action::Trade, Action::Fill, Action::None] {
            let update = books
                .apply(&message(6, 99, action, Side::None, PRICE, 1))
                .expect("non-book action");
            assert!(!update.state_changed);
        }

        books
            .apply(&message(7, 1, Action::Cancel, Side::Bid, PRICE, 3))
            .expect("full cancel");
        assert!(books.top(key).expect("valid").bid.is_none());
        books
            .apply(&message(8, 0, Action::Clear, Side::None, 0, 0))
            .expect("clear");
        assert_eq!(books.top(key).expect("valid"), TopOfBook::default());
    }

    #[test]
    fn rejects_out_of_order_and_inconsistent_events() {
        let mut books = valid_book();
        books
            .apply(&message(5, 1, Action::Add, Side::Bid, PRICE, 2))
            .expect("add");
        assert!(matches!(
            books.apply(&message(4, 1, Action::Cancel, Side::Bid, PRICE, 1)),
            Err(BookError::OutOfOrder { .. })
        ));
        assert!(matches!(
            books.apply(&message(6, 2, Action::Cancel, Side::Bid, PRICE, 1)),
            Err(BookError::UnknownOrder { .. })
        ));
        assert!(matches!(
            books.apply(&message(6, 1, Action::Cancel, Side::Bid, PRICE, 3)),
            Err(BookError::OverCancel { .. })
        ));
    }

    #[test]
    fn snapshot_only_becomes_valid_at_event_boundary() {
        let mut books = BookSet::default();
        let mut clear = message(1, 0, Action::Clear, Side::None, 0, 0);
        clear.flags = FlagSet::empty().set_snapshot();
        assert!(!books.apply(&clear).expect("clear").book_valid);
        let mut add = message(2, 1, Action::Add, Side::Bid, PRICE, 2);
        add.flags = FlagSet::empty().set_snapshot().set_last();
        assert!(books.apply(&add).expect("snapshot add").book_valid);
    }
}
