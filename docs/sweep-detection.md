# Liquidity sweep detection

This detector is a transparent market-data heuristic, not a trading signal or an execution recommendation. It runs on MBO trade records only after the reconstructed book has a complete clear/snapshot baseline.

## Parameters

The committed parameter set is `config/sweep.json` and has four free parameters.

| Parameter | Value | Meaning |
| --- | ---: | --- |
| `lookback_trades` | 50 | Prior trades defining the rolling swing high and low |
| `threshold_ticks` | 4 | Required penetration beyond the prior extreme |
| `reversion_window_ms` | 5000 | Maximum time to trade back through the swept level |
| `tick_size` | 250000000 | ES tick size in DBN fixed-price units (0.25 index points) |

For each publisher-qualified instrument, a trade at least 4 ticks beyond the prior 50-trade high or low opens at most one candidate in that direction. A later trade must cross back through the swept level within 5000 milliseconds. The emitted event records the initial sweep time, reversion time, maximum displacement, duration, swept level, and visible resting size attributable to the triggering trade.

`resting_size_consumed` is deliberately conservative: it is the smaller of the pre-event displayed quantity at the swept level and the triggering trade quantity. It does not estimate hidden liquidity or claim that every contract in a multi-level execution was consumed at that one level.

## Verified run

- Provenance: `live`; dataset `GLBX.MDP3`; symbol `ES.v.0`; interval `2025-04-03T22:00:00Z` through `2025-04-04T21:00:00Z`.
- Scanned 33438049 MBO records.
- Emitted 11 monotonic events: 7 above prior highs and 4 below prior lows.

The reproducible JSONL output is `out/sweeps.jsonl` and remains a generated local artifact.

Run it with:

```sh
cargo run --release --locked -p dbn-es-bench --bin dbn-es-bench -- analyze sweeps --manifest data/manifest.json --config config/sweep.json --output out/sweeps.jsonl --summary data/sweep-summary.json --report docs/sweep-detection.md
```
