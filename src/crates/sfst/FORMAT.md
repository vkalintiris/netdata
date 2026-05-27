# SFST File Format

SFST is the on-disk format for one log-index file. Each file holds the
indexed contents of one log stream (one
`(service.namespace, service.name)` pair) and is built from one WAL
file by the `sfst::indexer` module. The container is chunk-based with
a `gix-chunk` table of contents; every chunk has a 4-byte id naming
its role.

This document defines the on-disk shape — the bytes, the chunk ids,
and the schema of each chunk's payload. Producers (the indexer) and
consumers (the query reader) sit on top of this format and are out of
scope here.

---

## File Layout

    ┌───────────────────────────────────────────────┐
    │  Header                       (12 bytes)      │
    ├───────────────────────────────────────────────┤
    │  TOC                          (gix-chunk)     │
    │    12 × (num_chunks + 1) bytes                │
    ├───────────────────────────────────────────────┤
    │  Chunk bodies                                 │
    │    Concatenated in TOC order.                 │
    └───────────────────────────────────────────────┘

The TOC immediately follows the header. Chunk bodies follow the TOC in
the same order their entries appear in it. Readers look chunks up by
id through the TOC and must not assume a positional layout.

---

## Header

    Offset  Size  Field        Encoding
    ──────  ────  ───────────  ────────────────────────
    0       4     magic        ASCII "SFST"
    4       4     version      u32 little-endian
    8       4     num_chunks   u32 little-endian

A reader rejects:

- any other magic with `Error::InvalidMagic`,
- any other version with `Error::UnsupportedVersion`,
- a file shorter than 12 bytes with `Error::FileTooShort`,
- a `num_chunks` value that exceeds the file body's plausible
  maximum (each TOC entry is at least 12 bytes) with `Error::Toc` —
  defense-in-depth against a corrupted header.

`num_chunks` is the number of chunk bodies between the TOC and EOF.
The TOC carries one entry per chunk plus a trailing sentinel.

---

## Table of Contents

A `gix_chunk::file::Index`. Each entry is 12 bytes:

    Bytes  Field
    ─────  ─────────────────────────────
    0..4   chunk id ([u8; 4])
    4..12  body offset within file (u64 LE)

The TOC has `num_chunks + 1` entries. The final entry is a sentinel
(`id = [0; 4]`) whose offset is EOF; chunk sizes are computed as the
delta between an entry's offset and the next entry's offset.

Chunk ids are 4 bytes. A producer must not emit the same id twice.

---

## Chunk Ids

Every chunk in the file uses one of the ids below. Singleton ids
identify a single chunk; indexed ids encode the chunk's position
within its tier in the trailing bytes.

    Id          Payload                                       Required?
    ──────────  ────────────────────────────────────────────  ──────────
    "SUMR"      Summary                                       No (always emitted)
    "META"      Metadata                                      No (always emitted)
    "PRIM"      FstIndex<BitmapValue>                         Yes
    "MF{hi}{lo}" FstIndex<BitmapValue>  (mid-card field)      No (one per mid field)
    "HF{hi}{lo}" HighField  (high-card field, columnar SoA)   No (one per high field)
    "TIMS"      Vec<i64>  (per-log nanosecond timestamps)     Yes
    "SB0{N}"    Vec<Vec<KvId>>  (stream-batch N, 0..=7)       Yes (at least 1)

Indexed ids:

- `"MF{hi}{lo}"` is the 4 bytes `[b'M', b'F', hi, lo]` where
  `index = (hi << 8) | lo`. The chunk holds the FST for the
  mid-cardinality field at that position in the file's tier-sorted
  field list.
- `"HF{hi}{lo}"` is the analogous 4 bytes `[b'H', b'F', hi, lo]` for
  the high-cardinality field at that position. Payload is a
  struct-of-arrays — *not* an FST, because FSTs compress poorly at
  high cardinality. See [§ `HF{i}`](#hfi--high-card-field-columnar)
  for the schema.
- `"SB0{N}"` is the 4 bytes `[b'S', b'B', b'0', b'0' + N]` for
  `N` in `0..MAX_STREAM_BATCHES` (currently 8). The trailing byte is
  an ASCII digit so the ids are human-readable when dumping the TOC.

Indices start at 0 and are contiguous within each tier. A producer
emitting `M` mid-card chunks uses ids `MF{0}` through `MF{M-1}`;
similarly for `HF{i}` and `SB{i}`.

`PRIM`, `TIMS`, and at least one `SB{i}` are required; a writer fails
with `Error::NoPrimary` / `Error::NoTimestamps` /
`Error::InvalidStreamBatchCount(0)` if any is missing. The other
named chunks are technically optional at the container level, but the
canonical producer always emits all of them.

---

## Stream-batch partitioning

The number of `SB{i}` chunks in a file is derived from the file's
total log count via a fixed rule — it is **not** stored anywhere in
the file. Both writer and reader compute it identically:

    pub const MIN_LOGS_PER_BATCH: u32 = 1024;
    pub const MAX_STREAM_BATCHES: u8 = 8;

    pub fn num_stream_batches(total_logs: u32) -> u8 {
        (total_logs / MIN_LOGS_PER_BATCH).clamp(1, MAX_STREAM_BATCHES as u32) as u8
    }

    pub fn stream_batch_size(total_logs: u32) -> u32 {
        if total_logs == 0 { 1 } else { total_logs.div_ceil(num_stream_batches(total_logs) as u32) }
    }

Properties:

- A file with `total_logs == 0` carries exactly one (empty) `SB00`
  chunk so the TOC always has at least one stream-batch entry.
- Files with `total_logs ≤ MIN_LOGS_PER_BATCH` (1024) use one batch.
- Files above that scale linearly until they hit `MAX_STREAM_BATCHES`
  (8), where the rule clamps. The 8-batch ceiling exists so the
  per-value batch-membership mask in each `HF{i}` chunk fits in a
  single `u8` (one bit per batch — see
  [§ `HF{i}`](#hfi--high-card-field-columnar)).

The writer partitions chronologically-sorted log positions into
`stream_batch_size(total_logs)`-sized contiguous slices and emits one
`SB{i}` chunk per slice. The reader, given a chronological position
`p`, finds its batch as `p / stream_batch_size(total_logs)`.

---

## Chunk Payloads

All payloads are `bincode + zstd` (see [§ Encoding](#encoding)). The
decoded type of each chunk is given below.

### `SUMR` — Summary

The cheap recovery summary. Decodes to:

    pub struct Summary {
        pub min_timestamp_s: u32,
        pub max_timestamp_s: u32,
        pub total_logs:      u32,
        pub stream:          ServiceStream,
    }

    pub struct ServiceStream {
        pub namespace: String,
        pub name:      String,
    }

`min_timestamp_s` and `max_timestamp_s` are the earliest and latest
log seconds (Unix epoch) in the file. `total_logs` drives the
stream-batch partitioning (see above). `stream` carries the file's
single `(service.namespace, service.name)` identity.

### `META` — Metadata

Heavy query-time metadata:

    pub struct Metadata {
        pub histogram: Histogram,
        pub id_ranges: IdRanges,
        pub fields:    Vec<FieldEntry>,
    }

    pub struct Histogram {
        pub timestamps: Vec<u32>,   // second-boundary timestamps
        pub counts:     Vec<u32>,   // cumulative log count at each boundary
    }

    pub struct IdRanges {
        pub low_end:  KvId,
        pub mid_end:  KvId,
        pub high_end: KvId,
    }

    pub struct FieldEntry {
        pub name:        String,
        pub cardinality: u32,
        pub tier:        FieldTier,    // Low | Mid | High
    }

`histogram` supports time-range narrowing: binary-search `timestamps`
for the position bounds covering a window, then clip bitmaps to that
range. `id_ranges` tells a reader which cardinality tier a `KvId`
belongs to (see [§ Tier-Aligned IDs](#tier-aligned-ids)). `fields`
is ordered low → mid → high (each tier internally sorted by field
name) and yields the count of mid-card and high-card fields, which
in turn determines how many `MF{i}` and `HF{i}` chunks are present.

### `PRIM` — Primary FST

`fst_index::FstIndex<BitmapValue>` containing every low-cardinality
`key=value` entry across all low-card fields.

    pub struct BitmapValue {
        pub desc: treight::Bitmap,
        pub data: Vec<u8>,
    }

The bitmap records the time-sorted log positions where the
`key=value` pair appears. Always present.

### `MF{i}` — Mid-card field FSTs

One chunk per mid-cardinality field, in the order those fields appear
in `Metadata::fields`. Same payload schema as `PRIM` — an
`FstIndex<BitmapValue>` whose keys are full `key=value` strings.

### `HF{i}` — High-card field columnar

One chunk per high-cardinality field. Payload is a struct-of-arrays
with parallel columns:

    pub struct HighField {
        pub keys:  Vec<String>,  // sorted lex by key
        pub masks: Vec<u8>,      // batch-membership bitmask per key
    }

`keys` is the field's `key=value` strings, sorted lexicographically.
`masks[i]` is a bitmask over the file's stream batches: bit `b` is
set iff `keys[i]` appears in stream batch `b`. The reader uses the
mask to skip stream batches the value isn't in when materialising
matching positions — without this index, every high-card filter
would have to scan every `SB{i}` chunk.

Why SoA rather than `Vec<(String, u8)>`: the dense, low-cardinality
`u8` column compresses better when it's contiguous, and the
back-references zstd uses for the string column tighten when string
data is uninterrupted. The wire format is byte-identical regardless
of whether the writer uses a borrowed view (`HighFieldRef<'a> {
keys: &'a [&'a str], masks: &'a [u8] }`) or the owned form — a
regression test in `indexer/fst_builder.rs` guards the cross-shape
invariant.

### `TIMS` — Per-log timestamps

`Vec<i64>` of nanosecond timestamps in chronological order,
parallel-indexed to the concatenation of every
[`SB{i}`](#sbi--stream-batch-N) chunk:
`timestamps[i]` is the nanosecond timestamp of the log whose attribute
list lives at global position `i` in the concatenated stream.

Per-log timestamps follow the OTel hierarchy: `time_unix_nano` →
`observed_time_unix_nano` → `ingestion_ns + row_offset` (the indexer
synthesizes a fallback if both OTel timestamps are absent so that
every log has a well-defined timestamp).

Required: a writer that omits this chunk fails with
`Error::NoTimestamps`. Downstream tooling (display, sub-second
filtering, time-of-event citation) relies on this chunk.

### `SB{i}` — Stream-batch N

`Vec<Vec<KvId>>` indexed by chronological log position **within this
batch**:

    entries[local_pos] = [kv_id_1, kv_id_2, ...]

Where `local_pos = global_pos - i * stream_batch_size(total_logs)`.
The reader concatenates every `SB{i}` chunk in id order to recover
the full chronological log stream.

Each `KvId` references a `key=value` pair via the tier-aligned id
space below. The reader walks an `SB{i}` chunk to materialize a log's
attributes after time-range filtering has selected positions.

---

## Tier-Aligned IDs

A `KvId` is a `u32` identifying a `key=value` pair within the file:

    pub struct KvId(pub u32);

IDs are assigned during writing by walking the tiers in order:

    KvId 0          .. low_end       Low-card pairs   (primary FST iteration order)
    KvId low_end    .. mid_end       Mid-card pairs   (per-field FST iteration order)
    KvId mid_end    .. high_end      High-card pairs  (per-field chunk order)

`low_end`, `mid_end`, `high_end` are carried in `Metadata::id_ranges`.

The `SB{i}` chunks store `KvId`s, not strings; resolving a `KvId` back
to its `key=value` requires checking which range it falls in and
looking up the corresponding entry in `PRIM`, an `MF{i}` chunk, or an
`HF{i}` chunk.

Tier assignment is stable for a given input: fields are sorted by
name within each tier, and entries within each per-field structure
follow the underlying serialized order. Two indexers given the same
WAL produce identical id assignments.

---

## Encoding

All chunk payloads go through:

    serialized = bincode::serde::encode(value, bincode::config::standard())
    payload    = zstd::encode(serialized, level)

The crate exposes [`pack`] and [`unpack`] helpers around this. The
zstd compression level is a caller parameter; the format does not
fix it.

There is no container-level integrity check. The zstd frame format
includes a content checksum that catches corruption within a chunk's
payload, but the header and TOC are unprotected.

[`pack`]: https://docs.rs/sfst/latest/sfst/fn.pack.html
[`unpack`]: https://docs.rs/sfst/latest/sfst/fn.unpack.html

---

## Format Version

The current version is **2**.

### v2 changelog (from v1)

- **`STRM` → `SB{i}` stream-batch chunks.** v1 stored every log's
  attribute list in a single `STRM` chunk; v2 partitions logs
  chronologically into 1–8 `SB{i}` chunks (`SB00`..`SB07`), one per
  partition. Partition count and size are derived from
  `Summary::total_logs` via `num_stream_batches` /
  `stream_batch_size` — see
  [§ Stream-batch partitioning](#stream-batch-partitioning).
  This enables a high-card filter to materialise only the batches
  its values appear in, rather than scanning the whole stream.
- **`HF{i}` payload reshape.** v1 stored each high-card field as
  `Vec<(String, BitmapValue)>`. v2 replaces it with a columnar
  `HighField { keys: Vec<String>, masks: Vec<u8> }`. `BitmapValue`
  is gone from the high-card path; `masks` is a per-value batch-
  membership bitmask that tells a reader which `SB{i}` chunk(s)
  contain the value. The change requires the batching above to
  exist, so the two are bumped together.

v1 files cannot be read by a v2 reader and vice versa
(`Error::UnsupportedVersion` on the version field). No migration tool
exists — v1 files were never deployed beyond development.

### When to bump the version

A bump is required for any change that breaks the on-disk contract:

- adding a required chunk id,
- removing an existing chunk id,
- changing any chunk's payload schema in a non-backwards-compatible way,
- changing the stream-batch partitioning rule (which would change
  every reader's view of which `SB{i}` chunk a position lives in),
- changing how the TOC is laid out.

Adding a new optional chunk id, or extending an existing payload with
a field whose default decodes from absent bytes, does not require a
bump.
