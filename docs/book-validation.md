# Book reconstruction validation

**Result:** 100.000000% end-to-end exact top-of-book match and 100.000000% exact match among precisely aligned updates, measured on live `ES.v.0` data.

## Provenance and method

- Dataset: `GLBX.MDP3`; source: `live`; interval: `2025-04-03T22:00:00Z` through `2025-04-04T21:00:00Z`.
- Tick size: `250000000` fixed-price units.
- Streaming merge groups records by exact (ts_event, publisher_id, instrument_id, sequence), then pairs only identical action, price, and size records within each group. Each MBO event is buffered through F_LAST: Trade records compare the reconstructed pre-event book, while state-changing Add, Cancel, Modify, and Clear records compare the final event book. MBO state is withheld until a complete clear/snapshot ending in F_LAST. Every MBP-10 update after that baseline remains in the end-to-end denominator; unmatched records are failures, not dropped observations.

## Measured coverage and accuracy

| Metric | Value |
| --- | ---: |
| MBO records scanned | 33438049 |
| MBP-10 records scanned | 24171039 |
| MBO records withheld before baseline/recovery | 300066 |
| MBP-10 updates before valid MBO baseline | 209423 |
| Aligned updates | 23961616 |
| Unmatched eligible MBO observations | 6622658 |
| Unmatched MBP-10 updates | 0 |
| Exact price-and-size matches | 23961616 |
| Price matches | 23961616 |
| Size matches | 23961616 |
| Alignment coverage | 100.000000% |
| Aligned exact match | 100.000000% |
| End-to-end exact match | 100.000000% |

## Discrepancy histogram

| Classification | Updates |
| --- | ---: |
| `exact` | 23961616 |

## Failure-mode analysis

The request begins mid-session, so reconstruction is intentionally withheld until Databento's complete daily MBO snapshot establishes a baseline. That is a correctness boundary, not a tuned exclusion. Exact-key misses after the baseline remain in the end-to-end denominator. MBO events are held through F_LAST because intermediate book state is not inspectable; MBP-10 trade snapshots use the pre-event book while mutations use final event state. Aligned price mismatches indicate reconstruction or source-semantic disagreement; size-only mismatches indicate aggregation disagreement even when prices align. Venue sequence values may repeat across normalized records, so alignment also requires timestamp, publisher, instrument, and record identity.

## First ten aligned discrepancies

No aligned discrepancies were observed.
