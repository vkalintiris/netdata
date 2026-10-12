# Per-key=value comparison: jetstream-250k

Universe 250000 rows; 24034 low/mid lists across 2048 fields.

| metric | treight | roaring | t/r |
|---|---|---|---|
| stored bytes (treight wire vs roaring optimized) | 1211140 | 1311699 | 0.923 |
| treight tree bytes only / roaring as built | 994834 | 1538624 | 0.647 |
| heap allocations to load all lists | 24029 | 102393 | 0.235 |
| heap bytes requested to load all lists | 994834 | 2330759 | 0.427 |
| lists where treight is smaller | 22433 of 24034 | | |

Roaring containers: 30284 total (30217 array, 39 bitset, 28 run). Treight complemented lists: 10.

## zstd over concatenated stored forms

| level | treight | roaring | t/r |
|---|---|---|---|
| level_1 | 352883 | 366083 | 0.964 |
| level_19 | 281063 | 309481 | 0.908 |
| level_3 | 327405 | 348623 | 0.939 |

## Workloads (fastest of 7)

| workload | treight ms | roaring ms | treight allocs (cold / warm) | roaring allocs |
|---|---|---|---|---|
| load all lists from bytes | 0.458 | 1.859 | 24029 | 102393 |
| AND window_2pct (721 tree B) + len, every list | 43.867 | 0.656 | 1 / 0 | 8411 |
| range_cardinality window_2pct, every list | 4.892 | 0.169 | 0 | 0 |
| AND window_50pct (17862 tree B) + len, every list | 956.018 | 2.781 | 1 / 0 | 36457 |
| range_cardinality window_50pct, every list | 4.890 | 0.165 | 0 | 0 |
| AND ~10% list (card 25735, 22471 tree B) + len, every list | 1330.021 | 1.370 | 1 / 0 | 38424 |
| OR all values per field + len (2048 fields) | 90.582 | 6.414 | 17 / 0 | 31813 |
