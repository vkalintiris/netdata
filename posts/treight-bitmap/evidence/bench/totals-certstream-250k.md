# Index-footprint totals — treight vs roaring

- corpus: `/home/vk/repos/nd/bp/data/corpus/certstream-250k.jsonl.gz` (universe 250000 rows, 2437940 posting lists, sha256 cf0af069bd41de60…)
- complement: 23 lists encoded complemented (>50% density), 2437917 direct

## SFST-materialized (low+mid)

| metric | treight | roaring | t/r ratio |
|---|---|---|---|
| serialized bytes | 1792162 | 3514248 | 0.510 |
| + 5B descriptor per list | 1797922 | — | — |
| in-memory heap | 1797922 | 4184992 | 0.430 |
| lists | 1152 | 1152 | — |

## all lists

| metric | treight | roaring | t/r ratio |
|---|---|---|---|
| serialized bytes | 20059490 | 51874054 | 0.387 |
| + 5B descriptor per list | 32249190 | — | — |
| in-memory heap | 32249190 | 475410320 | 0.068 |
| lists | 2437940 | 2437940 | — |

## Per density bucket

| bucket | lists | treight B | roaring B | t/r |
|---|---|---|---|---|
| 0.001-0.01% | 195307 | 3107080 | 6064678 | 0.512 |
| 0.01-0.1% | 4903 | 814796 | 831554 | 0.980 |
| 0.1-1% | 182 | 340542 | 331392 | 1.028 |
| 1-10% | 87 | 534196 | 1370700 | 0.390 |
| 10-50% | 34 | 768574 | 1115472 | 0.689 |
| <=0.001% | 2237404 | 14348533 | 41405674 | 0.347 |
| >=50% | 23 | 145769 | 754584 | 0.193 |
