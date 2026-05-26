# SFST File Format

SFST is the on-disk format for one log-index file. Each file holds the
indexed contents of one log stream (one
`(service.namespace, service.name)` pair) and is built from one WAL
file by the indexer in `log-index`. The container is chunk-based with
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
- a file shorter than 12 bytes with `Error::FileTooShort`.

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
within its tier in the trailing two bytes.

    Id          Payload                                  Required?
    ──────────  ───────────────────────────────────────  ──────────
    "SUMR"      Summary                              No (always emitted)
    "META"      Metadata                            No (always emitted)
    "PRIM"      FstIndex<BitmapValue>                    Yes
    "MF{hi}{lo}" FstIndex<BitmapValue>  (mid-card field) No
    "HF{hi}{lo}" Vec<(String, BitmapValue)>  (high-card) No
    "TIMS"      Vec<i64>  (per-log nanosecond timestamps) Yes
    "STRM"      Vec<Vec<KvId>>  (stream log entries)     No (always emitted)

Indexed ids:

- `"MF{hi}{lo}"` is the 4 bytes `[b'M', b'F', hi, lo]` where
  `index = (hi << 8) | lo`. The chunk holds the FST for the
  mid-cardinality field at that position in the file's tier-sorted
  field list.
- `"HF{hi}{lo}"` is the analogous 4 bytes `[b'H', b'F', hi, lo]` for
  the high-cardinality field at that position. The payload is a
  sorted list of `(key=value, bitmap)` pairs — *not* an FST, because
  FSTs compress poorly at high cardinality.

Indices start at 0 and are contiguous within each tier. A producer
emitting `M` mid-card chunks uses ids `MF{0}` through `MF{M-1}`;
similarly for `HF{i}`.

`PRIM` and `TIMS` are required; a writer fails if either isn't set.
The other named chunks are technically optional at the container
level, but the canonical producer always emits all of them.

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
log seconds (Unix epoch) in the file. `total_logs` is the count of
log records. `stream` carries the file's single
`(service.namespace, service.name)` identity.

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
in `FLDS`. Same payload schema as `PRIM` — an
`FstIndex<BitmapValue>` whose keys are full `key=value` strings.

### `HF{i}` — High-card field sorted lists

One chunk per high-cardinality field. Payload is
`Vec<(String, BitmapValue)>` sorted by key. Not an FST.

### `TIMS` — Per-log timestamps

`Vec<i64>` of nanosecond timestamps in chronological order,
parallel-indexed to [`STRM`](#strm--stream-log-entries):
`timestamps[i]` is the nanosecond timestamp of the log whose attribute
list lives at `entries[i]`.

Per-log timestamps follow the OTel hierarchy: `time_unix_nano` →
`observed_time_unix_nano` → `ingestion_ns + row_offset` (the indexer
synthesizes a fallback if both OTel timestamps are absent so that
every log has a well-defined timestamp).

Required: a writer that omits this chunk fails with
`Error::NoTimestamps`. Downstream tooling (display, sub-second
filtering, time-of-event citation) relies on this chunk.

### `STRM` — Stream-log-entries

`Vec<Vec<KvId>>` indexed by chronological log position:

    entries[time_sorted_pos] = [kv_id_1, kv_id_2, ...]

Each `KvId` references a `key=value` pair via the tier-aligned id
space below. The reader walks this chunk to materialize a log's
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

The `STRM` chunk stores `KvId`s, not strings; resolving a `KvId` back
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

The current version is **1**.

A bump is required for any change that breaks the on-disk contract:

- adding a required chunk id,
- removing an existing chunk id,
- changing any chunk's payload schema in a non-backwards-compatible way,
- changing how the TOC is laid out.

Adding a new optional chunk id, or extending an existing payload with
a field whose default decodes from absent bytes, does not require a
bump.
