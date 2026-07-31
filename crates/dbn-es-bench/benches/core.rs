use std::hint::black_box;
use std::time::Duration;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use dbn::{Action, FlagSet, MboMsg, Side};
use dbn_es_core::BookSet;

const MESSAGE_COUNT: usize = 1_000;

fn message(sequence: u32, order_id: u64, action: Action, side: Side, price: i64) -> MboMsg {
    let mut message = MboMsg::default();
    message.hd.publisher_id = 1;
    message.hd.instrument_id = 2;
    message.hd.ts_event = u64::from(sequence);
    message.order_id = order_id;
    message.action = i8::try_from(u8::from(action)).expect("DBN actions fit in i8");
    message.side = i8::try_from(u8::from(side)).expect("DBN sides fit in i8");
    message.price = price;
    message.size = 1;
    message.flags = FlagSet::empty().set_last();
    message.sequence = sequence;
    message
}

fn fixture() -> (BookSet, Vec<MboMsg>) {
    let mut books = BookSet::default();
    books
        .apply(&message(0, 0, Action::Clear, Side::None, 0))
        .expect("clear establishes the snapshot baseline");
    let messages = (0..MESSAGE_COUNT)
        .map(|index| {
            let sequence = u32::try_from(index + 1).expect("fixture is bounded");
            let order_id = u64::from(sequence);
            let side = if index % 2 == 0 { Side::Bid } else { Side::Ask };
            let offset = i64::try_from(index % 20).expect("fixture is bounded") * 250_000_000;
            message(
                sequence,
                order_id,
                Action::Add,
                side,
                5_000_000_000_000 + offset,
            )
        })
        .collect();
    (books, messages)
}

fn order_book(c: &mut Criterion) {
    let mut group = c.benchmark_group("order_book");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(1));
    group.throughput(Throughput::Elements(
        u64::try_from(MESSAGE_COUNT).expect("fixture is bounded"),
    ));
    group.bench_function("apply_1000_adds", |bencher| {
        bencher.iter_batched(
            fixture,
            |(mut books, messages)| {
                for message in &messages {
                    black_box(books.apply(message).expect("fixture must remain valid"));
                }
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(benches, order_book);
criterion_main!(benches);
