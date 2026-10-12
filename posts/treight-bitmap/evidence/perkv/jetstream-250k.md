# Per-key=value comparison: jetstream-250k

Universe 250000 rows; 24034 low/mid lists across 2048 fields.

| metric | treight | roaring | t/r |
|---|---|---|---|
| stored bytes (treight wire vs roaring optimized) | 1211140 | 1311699 | 0.923 |
| treight tree bytes only / roaring as built | 994834 | 1538624 | 0.647 |
| allocator calls to load all lists (incl. transient) | 24029 | 102393 | 0.235 |
| heap bytes requested to load all lists (incl. transient) | 994834 | 2330759 | 0.427 |
| live heap blocks after loading all lists | 24029 | 54318 | 0.442 |
| live heap bytes after loading all lists | 994834 | 2088480 | 0.476 |
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
| load all lists from bytes | 0.440 | 1.866 | 24029 | 102393 |
| AND window_2pct (721 tree B) + len, every list | 44.108 | 0.500 | 1 / 0 | 8411 |
| range_cardinality window_2pct, every list | 4.802 | 0.146 | 0 | 0 |
| AND window_50pct (17862 tree B) + len, every list | 967.756 | 2.840 | 1 / 0 | 36457 |
| range_cardinality window_50pct, every list | 4.795 | 0.175 | 0 | 0 |
| AND ~10% list (card 25735, 22471 tree B) + len, every list | 1375.013 | 1.514 | 1 / 0 | 38424 |
| OR all values per field + len (2048 fields) | 88.122 | 6.870 | 17 / 0 | 31813 |
