# dbengine codec vectors (step S0)

Golden vectors for the dbengine on-disk page codecs, produced by the C implementation of netdata/netdata @ 1e97a0fc9e.

- Nothing here re-implements a C encoder or decoder:
  - `gorilla.cc` runs from the build's `liblibnetdata.a`;
  - `src/database/engine/page.c` is compiled from the reference tree and linked with `gen/oracle-stubs.c`;
  - `src/database/engine/page_test.cc` is compiled verbatim against a stand-in gtest header (`gen/fake-gtest/`).
- No `netdata` binary is run.
- The one replicated piece of logic is the buffer growth of `pgd_append_point` (page.c:963-976), used for the
  writer-level cases whose inputs are arbitrary u32 values. The page-level cases call `pgd_append_point` itself, and
  the generator aborts unless both paths produce the same bytes.

## Regenerate

```sh
NETDATA_SRC=<netdata tree at 1e97a0fc9e> NETDATA_BUILD=<its CMake build dir> [VECTORS_WORK=<scratch dir>] gen/gen.sh
```

- Compiler flags and libraries come from the shared C-oracle helper `src/crates/netdata-agent/c-oracle/lib.sh`
  (located with `git rev-parse --show-toplevel`), the same one `storage/tests/oracle/gen-vectors.sh` uses.
- It needs gcc/g++ with LTO: `liblibnetdata.a` holds `-flto -fno-fat-lto-objects` objects.
- Deterministic: two full runs gave byte-identical output (`cmp`).
- The script aborts if the three `page_test.cc` strings it rewrites for the variants have changed.
- Without `VECTORS_WORK`, build outputs go to a temporary directory that is removed on exit.

## Files

| File | Contents | C source it reflects | Spec (knowledge/spec-dbengine.md) |
|---|---|---|---|
| `gorilla-writer.json` | 26 codec cases: u32 input, buffer chain bytes, write failures, both decodes | gorilla.cc:109-439; page.c:963-976 | §3.2; B1 §6.3 |
| `gorilla-page.json` | 10 GORILLA_32BIT pages through the real `pgd_*` API, with cursor points | page.c:471-1151 | §3.2; B1 §6.3-6.5 |
| `gorilla-disk-load.json` | 24 crafted inputs to `pgd_create_from_disk_data`, with the result and cursor points | page.c:522-576; gorilla.cc:285-315,353-439 | B1 §6.5 |
| `raw-pages.json` | 8 ARRAY_32BIT / ARRAY_TIER1 pages and disk inputs | page.c:500-566,980-1004,1114-1137 | §3.3-3.4; B1 §6.2 |
| `pgd-tests.json` | hand extraction of the 11 `page_test.cc` tests: inputs, operations, EXPECTs, functions | page_test.cc:1-477 | B1 §6.4-6.5 |
| `pgd-tests-run.json` | results of running `page_test.cc` (4 builds) against the real page.c | page_test.cc; page.c | B1 §6.4-6.5 |
| `gen/gen.sh` | driver: builds and runs everything | | |
| `gen/gen-gorilla.cc` | generator of the four codec files | | |
| `gen/oracle-stubs.c` | 9 link stubs; none affects encoded bytes (see the file header) | rrdengineapi.c:28-42 | |
| `gen/page-test-compat.h` | force-included when compiling `page_test.cc`: old page-type names, `pgc_destroy` arity | | |
| `gen/pgd-tests-main.cc` | `main` calling `pgd_test()`, as `-W pgd-tests` does (main.c:432-433) | | |
| `gen/fake-gtest/gtest/gtest.h` | fork-per-test gtest stand-in that records every EXPECT in shared memory | | |

## Conventions

- **u32 values**: strings `"0x%08x"`.
- **Doubles**: `{"bits": "<16 hex digits of the IEEE-754 binary64>", "repr": "%.17g"}`. NaN and Inf are exact only in
  `bits`.
- **Flags**: decimal. `SN_FLAG_NOT_ANOMALOUS` = 16777216 (bit 24), `SN_FLAG_RESET` = 33554432 (bit 25).
- **Buffer hex** (`hex_next_masked`):
  - 512 bytes in memory order (little-endian fields), with bytes 0-7 (`next`) zeroed; `next` is given separately as
    `"nonzero"` or `"zero"`.
  - C stores the heap address of the next in-memory buffer there (gorilla.cc:136, copied verbatim by
    `gorilla_writer_serialize`, gorilla.cc:265-283). The reader only tests it for zero (gorilla.cc:293) and overwrites
    it (gorilla.cc:303).
  - To rebuild the exact disk bytes, put any non-zero u64 in `next` of every buffer except the last.
- **`input_hex`** (disk-load): the exact bytes passed, with every non-zero `next` set to the canonical value 1 (LE u64).
- **Point tuple** `[ok, sum_bits, min_bits, max_bits, count, anomaly_count, flags]`:
  - `ok` is the return value of `pgdc_get_next_point`; the rest are the `STORAGE_POINT` fields.
  - When `ok` is false the point is `storage_point_empty`: NaN, count 1, anomaly 0, flags 0 (storage-point.h:32-39).
  - `points_from_0[i]` is the i-th call after `pgdc_reset(&c, pg, 0)`. The list reads one position past `used`, so its
    last entry shows the end-of-page result.
- Every file starts with `constants`:
  - `buffer_size` 512, `buffer_slots` 128;
  - `sizeof_gorilla_header_t` 16 and `capacity_bits` 3968 (64-bit build);
  - `canonical_next` 1, `SN_EMPTY_SLOT`, `SN_DEFAULT_FLAGS`.

## Schemas

### gorilla-writer.json `cases[]`

- `name`, `description`, `count`, `input_u32[]`.
- `write_failures[]`: every `gorilla_writer_write` that returned false. Each entry holds:
  - `index` (the value that did not fit) and `full_buffer`;
  - `nbits_before` and `nbits_after`: the full buffer's nbits before and after the failed write;
  - `partial_bits` = `nbits_after - nbits_before`, the bits the failed write left behind (0, 1, 2 or 7);
  - `entries` of the full buffer.
  - The value was then written raw into a new zeroed buffer.
- `num_buffers`, `byte_length` (`num_buffers × 512`).
- `writer_entries`, `actual_nbytes`, `optimal_nbytes`: `gorilla_writer_entries/actual_nbytes/optimal_nbytes`.
  `optimal_nbytes` feeds only pulse statistics (page.c:845-848).
- `serialize_into_size_minus_1`: `gorilla_writer_serialize` into one byte too few. It is always false.
- `buffers[]`: `{next, entries, nbits, hex_next_masked}`.
- `decoded_live[]`: `gorilla_writer_get_reader` + `gorilla_reader_read` until false.
- `disk_patch_ok`, `disk_patch_entries`, `decoded_disk[]`: `gorilla_buffer_patch(copy, num_buffers)`, then
  `gorilla_reader_init` + `gorilla_reader_read` until false. The generator aborts unless both decodes equal the input.
- Optional fields: `spec_claim` + `spec_claim_confirmed` (`spec_example_5_5_6`), `failing_check` (`fail_*` cases),
  `note`.
- Cases:
  - `spec_example_5_5_6`, `empty`, `single_value_666`, `zero_first_value`, `lzc_zero_equals_initial_prev_lzc`,
    `lzc_31_single_bit_xor`;
  - `all_equal_1024`, `constant_runs`, `monotonic_raw_u32_0_1023`, `monotonic_sn_0_1023`, `alternating_1_2`;
  - `random_mantissa24_1024` (splitmix64 seed 0x676f72696c6c6131, `0x01000000 | rnd() & 0xFFFFFF`);
  - `random_u32_1024` (seed 0x676f72696c6c6132);
  - `adversarial_lzc_alternating_1024` and `_1100`;
  - `boundary_second_buffer`, `boundary_third_buffer`;
  - the six `fail_*` cases, one per capacity check of `gorilla_writer_write` (gorilla.cc:176,185,194,201,208 twice),
    each followed by 3 values showing the per-buffer state reset;
  - `empty_slot_runs`, `all_empty_slot_1024`, `flags_mix_32`.

### gorilla-page.json `cases[]`

- `name`, `description`, `slots` (passed to `pgd_create`), `count`, `input_values[]`, `input_flags[]`.
- `packed_u32[]`: `pack_storage_number(n, flags)`.
- `append_returned_buffer_size_at[]`: indices where `pgd_append_point` returned 512 (a buffer was added, page.c:975).
- `pgd_is_empty`, `pgd_slots_used`, `collector_points_from_0[]`: cursor over the collector page, read before the flush.
- `disk_footprint`: `pgd_disk_footprint`.
- `buffers[]`, `num_buffers`, `matches_writer_encoding`: the bytes from `pgd_copy_to_extent`. They are absent when the
  footprint is 0.
- `from_disk`: `"PGD_EMPTY"` or `{pgd_slots_used, pgd_capacity, pgd_is_empty, points_from_0[]}` from
  `pgd_create_from_disk_data`.
- Cases:
  - `spec_example_5_5_6`, `copy_to_extent_666`, `sequence_0_1023`, `sequence_0_15`, `empty_page`;
  - `nan_gaps_64`, `all_nan_16`, `flags_and_edges`;
  - `disk_footprint_random_16_seed5489` and `_192_`: the page_test DiskFootprint inputs with `std::mt19937(5489)`.

### gorilla-disk-load.json `cases[]`

- `name`, `description`, `input_hex`, `size`: the size argument, which may differ from the byte count.
- `result`: `"PGD_EMPTY"` or `{pgd_slots_used, pgd_capacity, points_from_0[]}`.
- Optional: `entries`, `not_executed_hazard`.
- Cases:
  - valid: `valid_sequence_0_15`, `valid_two_buffers`, `valid_two_buffers_next_pointer_like`, `all_zero_512`,
    `eleven_buffers_5632`;
  - rejected: `reject_last_buffer_next_nonzero`, `reject_last_of_two_next_nonzero`, `reject_truncated_chain`,
    `reject_nbits_4096`, `reject_nbits_3968`, `reject_second_buffer_nbits_3968`, `reject_size_0`, `reject_size_3`;
  - boundary: `accept_nbits_3967`;
  - corrupt entries: `entries_plus_one`, `entries_zero`, `entries_u16_truncation_65537`/`_65536`,
    `entries_inflated_first_of_two`, `entries_deflated_first_of_two`;
  - short chains: `first_next_zero_of_two`, `middle_next_zero_of_three`;
  - odd sizes: `size_516_not_multiple_of_512`, `size_256_one_value_next_zero`.

### raw-pages.json `cases[]`

- `name`, `page_type` (`ARRAY_32BIT` = 0 / `ARRAY_TIER1` = 1), `page_type_id`, `description`.
- Collector cases hold:
  - `slots`, the input (`input_values`/`input_flags`, or `input[]` of `{sum, min, max, count, anomaly_count}`);
  - `pgd_is_empty`, `pgd_slots_used`, `disk_footprint`, `page_hex`;
  - tier 1 only: `records[]` `{sum_f32, min_f32, max_f32, count, anomaly_count, hex}`.
- Disk-only cases hold `disk_input_hex` and `size`.
- All cases have `from_disk` (`"PGD_EMPTY"` or `{pgd_slots_used, points_from_0}`).

### pgd-tests.json

- `facts`, `exercised_functions` (with page.c/gorilla.cc lines) and `append_signature`.
- `tests[]`, each with `test`, `lines`, `page_types`, `inputs`, `operations[]` (with lines), `expects[]`
  (`line`, `macro`, `expr`, `expected`, `note`), `exercises`, and `materialized` (the vector case that holds the
  concrete bytes).

### pgd-tests-run.json

- `variants[]`: `{variant, source_transform, tests[]}`.
- Each test has:
  - `outcome`: `passed`, `failed` (an EXPECT failed) or `died` (exit status or signal);
  - `last_expect_line`;
  - `stderr_tail`: netdata `msg=` fields only, with the thread-dependent ARAL partition masked as `N`;
  - `expects[]`: per source line, `evaluations`, `passed`, `failed`, `last_value`, `first_failure`.
- Variants:
  - `as_written`: verbatim.
  - `slots10240`: `slots_for_page(1024 * 1024)` becomes `slots_for_page(10240)` and `std::mt19937` is seeded with
    5489. 10240 is the smallest multiple of 1024 for which the Roundtrip loops (page_test.cc:355-365) run and end.
  - `u16max`: the same with 65535.
  - `array32`: `slots10240` plus `page_type = PAGE_METRICS`. It runs only the 6 tests that are meaningful and
    terminate: the gorilla-offset tests make out-of-bounds stack accesses, and Roundtrip never ends with 1024 slots.

## Facts found (read in code; reference tree paths)

### Test infrastructure

- **No gtest in the build:** `HAVE_GTEST` is defined nowhere.
  - The only hits are page_test.cc:4,467,477. The config header, CMake cache, CMakeLists and the page_test.cc compile
    command in `compile_commands.json` have no definition.
  - So `-W pgd-tests` runs the stub (page_test.cc:469-475): it prints a message and returns 0.
  - Commit 2d974dc51d says so too.
- **`page_test.cc` does not compile with `HAVE_GTEST`:**
  - `PAGE_METRICS` / `PAGE_GORILLA_METRICS` are undefined. They were renamed in 00f897a883 (rrddiskprotocol.h:43-46).
  - `pgc_destroy` takes 2 arguments since 932bbf3b4e (cache.h:178, page_test.cc:462).
- **Even when compiled**, 7 of the 11 tests die at their first append:
  - `pgd.used`, `pgd.slots` and `page_raw_t.size` are `uint16_t` (page.c:22,32,35) since 6b8c6baac2.
  - `slots_for_page(1024*1024)` therefore becomes 0, and the append hits "attempted to write beyond page size"
    (page.c:951-953).
  - EmptyOrNull and the three corrupt-disk tests pass (`as_written` variant).
- **`EXPECT_DEATH(pgd_disk_footprint(pg_disk))` (page_test.cc:353) fails** in this build. It relies on an
  `internal_fatal` (page.c:873-874), and `NETDATA_INTERNAL_CHECKS` is off (nd_log-fatal.h:13-17). Every other
  expectation passes in `slots10240`.
- **`slots_for_page(1)`** in two tests works only because `internal_fatal(slots == 1)` (page.c:483) is a no-op.

### Gorilla writer and page layout

- **Test coverage:** gorilla has no unit tests.
  - It has only a libFuzzer entry point (`ENABLE_FUZZER`, gorilla.cc:463-587) and a benchmark (`ENABLE_BENCHMARK`,
    gorilla.cc:589-658).
  - Both are built only by `fuzzer.sh` / `benchmark.sh`, never by CMake.
- **Capacity and bit order:**
  - Capacity is `128·32 − sizeof(gorilla_header_t)·8` = 3968 bits on 64-bit (gorilla.cc:32-34).
  - Every check is `nbits + k >= capacity` (gorilla.cc:164,176,185,194,201,208), so nbits never exceeds 3967.
  - Bits are LSB-first. A write at word offset 0 stores the whole word; other writes OR in and store the spill
    (gorilla.cc:55-79).
- **Failed writes leave partial bits.** A failed write can leave 1, 2 or 7 bits (a 0 control bit, the lzc flag, the
  5-bit lzc) with `entries` unchanged. The value is rewritten raw in a new zeroed buffer (page.c:965-972), and the
  state resets there (prev = 0, prev_lzc = 0, gorilla.cc:131-132). The `fail_*` cases pin each variant byte-exactly.
- **Initial `prev_xor_lzc` is 0**, so a first xor with lzc 0 is coded as "same lzc" without the 5-bit field
  (`lzc_zero_equals_initial_prev_lzc`).
- **Page size:** a tier-0 page holds at most 1024 points (4096 / 4, rrdengineapi.c:31,461-462).

### Loading pages from disk

- **Reader limits:**
  - `validate_page` accepts a gorilla `page_length` up to 4096 + 2·512 = 5120 (pdc.c:921-931).
  - Compressed extents also reject a gorilla page with `4096 < len` and `(len − 4096) % 512 ≠ 0` (pdc.c:1147-1152).
  - `pgd_create_from_disk_data` itself has no length limit (`eleven_buffers_5632` is accepted).
- **Chain and entries checks** in `pgd_create_from_disk_data` + `gorilla_buffer_patch`, all executed:
  - `size < 4` → PGD_EMPTY;
  - `nbuffers = size / 512`;
  - each visited buffer needs `nbits < 3968`;
  - following a non-zero `next` past `nbuffers` → PGD_EMPTY;
  - a zero `next` ends the chain early;
  - `used = Σ entries`, truncated to u16;
  - a size not a multiple of 512 is only an `internal_fatal` (page.c:537).
- **Decoding stops at a buffer's bit bound.** Inflated `entries` never let the reader advance to the next buffer, so
  the later buffers' values are lost (gorilla.cc:368-391). Deflated entries skip values.
- **Empty pages from disk:** a page loaded from disk never gets `PAGE_OPTION_ALL_VALUES_EMPTY` (page.c:530).
  `pgd_is_empty` is true for an all-NaN collector page and false for the same page reloaded (`all_nan_16`).
- **ARRAY pages from disk:**
  - `used = size / point_size`, so trailing bytes are ignored (`array32_size_10`, `tier1_size_20`).
  - The cursor returns every slot with `ok = true`, empty slots included (page.c:1114-1137).
  - For tier 1, flags = `anomaly_count ? 0 : NOT_ANOMALOUS` (page.c:1118).

## Inferences (not executed)

- **Upper bound of 10 buffers for 1024 values:**
  - Each non-first value costs `2 + 5·[lzc ≠ prev_lzc] + 32 − lzc` bits. Alternating lzc 1,0,1,… maximises every
    prefix, so a full buffer holds at least 103 values (observed: 103).
  - Hence 1024 values need at most ⌈1024/103⌉ = 10 buffers = 5120 B, exactly the `validate_page` limit.
- **`size < 512` with a non-zero `next`:**
  - `nbuffers` is 0 and `buffers` starts at 1, so the bound at gorilla.cc:294 never fires. The walk goes out of bounds.
  - Likewise, `nbits > (size−16)·8` lets the reader read past the copy.
  - Only the in-bounds sub-case was run (`size_256_one_value_next_zero`).
  - It is reachable only through a corrupt descriptor: `validate_page` accepts any `0 < page_length ≤ 5120`.
- **32-bit builds** have a 12-byte `gorilla_header_t` (4-byte `next`), data at offset 12 and 4000 capacity bits
  (gorilla.h:16-25, gorilla.cc:32-34). Their gorilla pages are laid out differently. These vectors are 64-bit only.
- **`raw.size` is `uint16_t`**, so a page ≥ 64 KiB is copied truncated (page.c:541-543). The `u16max` Roundtrip shows
  this (used 3635 instead of 65535). It is unreachable in production (≤ 5120 B).

## Spec cross-check

Confirmed:

- **B1 §6.3 worked example:** `spec_claim_confirmed: true`.
  - `pack(5)` = 0x314C4B40 and `pack(6)` = 0x315B8D80.
  - entries 3, nbits 61, `data[0]` 0x314C4B40, `data[1]` 0x17C6C059.
  - The first 28 bytes match.
- **B1 §6.3 sizes:**
  - 1024 constant values fit in 1 buffer (nbits 1055).
  - Random 24-bit mantissas need 8 buffers.
  - The adversarial LZC-alternating case needs 10 buffers = 5120 B (103 values per buffer).
  - Also observed: full-range random u32 needs 10 buffers.
- **§3.4 / B1 §6.2 tier-1 bytes:**
  - `{10,1,5,3,0}` = `00 00 20 41 00 00 80 3f 00 00 a0 40 03 00 00 00`.
  - NaN is stored as 0x7FC00000.
- **B1 §6.5 rules**, all executed: every-buffer nbits check, chain bound, early stop on zero `next`, u16 truncation,
  no descriptor cross-check, quiet stop at the bit bound.

Disagreements and additions:

1. **Pointer width.** §3.2 and B1 §6.3 give `next` as `u64 @0`. That holds only for 64-bit builds (see Inferences).
2. **Missing cases in B1 §6.5:**
   - the `size < 512` (nbuffers = 0) hazard;
   - inflated entries losing later buffers;
   - disk-loaded pages never being "all values empty".
3. **Stale gtest suite.** The spec does not mention that the `pgd-tests` suite cannot compile or run (see Facts), so it
   is not a usable oracle as it stands.

## storage_number vectors already in the Rust tree

`netdata-agent/storage/tests/vectors/{pack,unpack}.tsv` exist (made by `tests/oracle/gen-vectors.{c,sh}`); this
directory does not duplicate them. What they do not cover:

- **`unit_test_storage` values.** `-W unittest` → `unit_test_storage` (src/daemon/unit_test.c:406-435) visits 378
  doubles (±k·1e-7·10^i, k = 1..9 accumulated, i = 0..20) with `SN_DEFAULT_FLAGS`.
  - `test_storage_number.c:40` adds `16.777218`.
  - 358 of these 379 values are not in `pack.tsv` (21 are, with matching results). Checked by bit pattern + flags with a
    throwaway program, not kept.
  - Those tests assert only round-trip accuracy ≤ 0.0001 % and `print_netdata_double` output (unit_test.c:193-238), not
    packed bits.
- **Read-side semantics** (`flags = v & SN_USER_FLAGS`, `anomaly = exists && !bit24`, empty → NaN with count 1,
  anomaly 0): not in the TSVs. They are in the point tuples of `gorilla-page.json` and `raw-pages.json`.
- **`storage_number_tier1_t` records:** not in the TSVs. They are in `raw-pages.json`.
- **Tier-1 and ARRAY_32BIT test vectors:** the C source has none.
  - `raw-pages.json` is produced by running page.c, not hand-derived.
  - The struct it reflects is storage_number.h:78-84 (16 B, no padding), written at page.c:980-993.

## Not extracted, and why

- **The random inputs of the as-written MemoryFootprint/DiskFootprint** come from `std::random_device` and cannot be
  reproduced. The variants and `gorilla-page.json` use seed 5489. libstdc++ (GCC 14.2) passes raw `std::mt19937`
  output through a full-range `uniform_int_distribution<uint32_t>`: checked on 100000 outputs.
- **`validate_page` and the extent-level checks** (pdc.c) were not executed: they need the extent/MRG machinery. The
  5120 B and compressed-length rules above are from code.
- **Out-of-bounds cases** (`size < 512` with non-zero `next`, oversized nbits on short pages) were not executed.
- **The array32 `Roundtrip`** is not in the run file: its result at the timeout is not reproducible. One run was killed
  by SIGALRM after 300 s with 1.26·10^9 failed EXPECT_TRUE evaluations at page_test.cc:366, and EXPECT_DEATH at line
  353 failed.
