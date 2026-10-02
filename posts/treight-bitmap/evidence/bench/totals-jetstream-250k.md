# Index-footprint totals — treight vs roaring

- corpus: `/home/vk/repos/nd/bp/data/corpus/jetstream-250k.jsonl.gz` (universe 250000 rows, 1350739 posting lists, sha256 ee48bb0de47d9821…)
- complement: 10 lists encoded complemented (>50% density), 1350729 direct

## SFST-materialized (low+mid)

| metric | treight | roaring | t/r ratio |
|---|---|---|---|
| serialized bytes | 994834 | 1538624 | 0.647 |
| + 5B descriptor per list | 1115004 | — | — |
| in-memory heap | 1115004 | 5905000 | 0.189 |
| lists | 24034 | 24034 | — |

## all lists

| metric | treight | roaring | t/r ratio |
|---|---|---|---|
| serialized bytes | 11046106 | 27392778 | 0.403 |
| + 5B descriptor per list | 17799801 | — | — |
| in-memory heap | 17799801 | 262065816 | 0.068 |
| lists | 1350739 | 1350739 | — |

## Per density bucket

| bucket | lists | treight B | roaring B | t/r |
|---|---|---|---|---|
| 0.001-0.01% | 65263 | 1674461 | 2450530 | 0.683 |
| 0.01-0.1% | 3664 | 647245 | 507478 | 1.275 |
| 0.1-1% | 181 | 385382 | 344800 | 1.118 |
| 1-10% | 13 | 98956 | 129644 | 0.763 |
| 10-50% | 6 | 139564 | 196848 | 0.709 |
| <=0.001% | 1281602 | 8015961 | 23435398 | 0.342 |
| >=50% | 10 | 84537 | 328080 | 0.258 |
