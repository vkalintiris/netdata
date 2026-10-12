# Per-key=value comparison: certstream-250k

Universe 250000 rows; 1152 low/mid lists across 33 fields.

| metric | treight | roaring | t/r |
|---|---|---|---|
| stored bytes (treight wire vs roaring optimized) | 1802530 | 2668315 | 0.676 |
| treight tree bytes only / roaring as built | 1792162 | 3514248 | 0.510 |
| allocator calls to load all lists (incl. transient) | 1140 | 6941 | 0.164 |
| heap bytes requested to load all lists (incl. transient) | 1792162 | 2796529 | 0.641 |
| live heap blocks after loading all lists | 1140 | 4592 | 0.248 |
| live heap bytes after loading all lists | 1792162 | 2769000 | 0.647 |
| lists where treight is smaller | 656 of 1152 | | |

Roaring containers: 3440 total (3103 array, 154 bitset, 183 run). Treight complemented lists: 23.

## zstd over concatenated stored forms

| level | treight | roaring | t/r |
|---|---|---|---|
| level_1 | 627860 | 832724 | 0.754 |
| level_19 | 542000 | 726814 | 0.746 |
| level_3 | 600795 | 807158 | 0.744 |

## Workloads (fastest of 7)

| workload | treight ms | roaring ms | treight allocs (cold / warm) | roaring allocs |
|---|---|---|---|---|
| load all lists from bytes | 0.082 | 0.665 | 1140 | 6941 |
| AND window_2pct (721 tree B) + len, every list | 11.207 | 0.304 | 1 / 0 | 1503 |
| range_cardinality window_2pct, every list | 8.444 | 0.081 | 0 | 0 |
| AND window_50pct (17862 tree B) + len, every list | 64.234 | 2.532 | 1 / 0 | 3757 |
| range_cardinality window_50pct, every list | 8.429 | 0.093 | 0 | 0 |
| AND ~10% list (card 25697, 20753 tree B) + len, every list | 92.630 | 1.153 | 1 / 0 | 3765 |
| OR all values per field + len (33 fields) | 88.490 | 4.181 | 17 / 0 | 225 |
