# Index-footprint totals — treight vs roaring

- corpus: `/home/vk/repos/nd/bp/data/corpus/mixed-250k.jsonl.gz` (universe 250000 rows, 1971504 posting lists, sha256 949751f90787a65c…)
- complement: 3 lists encoded complemented (>50% density), 1971501 direct

## SFST-materialized (low+mid)

| metric | treight | roaring | t/r ratio |
|---|---|---|---|
| serialized bytes | 2111260 | 3353048 | 0.630 |
| + 5B descriptor per list | 2212840 | — | — |
| in-memory heap | 2212840 | 7409080 | 0.299 |
| lists | 20316 | 20316 | — |

## all lists

| metric | treight | roaring | t/r ratio |
|---|---|---|---|
| serialized bytes | 16392597 | 41542622 | 0.395 |
| + 5B descriptor per list | 26250117 | — | — |
| in-memory heap | 26250117 | 384317488 | 0.068 |
| lists | 1971504 | 1971504 | — |

## Per density bucket

| bucket | lists | treight B | roaring B | t/r |
|---|---|---|---|---|
| 0.001-0.01% | 120089 | 2229841 | 4045770 | 0.551 |
| 0.01-0.1% | 3991 | 556535 | 545218 | 1.021 |
| 0.1-1% | 317 | 488610 | 460290 | 1.062 |
| 1-10% | 101 | 639395 | 1474162 | 0.434 |
| 10-50% | 26 | 539142 | 680808 | 0.792 |
| <=0.001% | 1846968 | 11739112 | 34016478 | 0.345 |
| >=50% | 12 | 199962 | 319896 | 0.625 |
