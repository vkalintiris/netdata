# SFST File Format

SFST is a chunked binary container. A file has a fixed-size header, a
`gix-chunk` table of contents, and a sequence of chunk bodies. Each
chunk is identified by a 4-byte id and carries an opaque payload —
typically `bincode` + `zstd`, but the container does not require that.

This document defines the format. Producers and consumers (indexers,
readers, registries) are out of scope; see their respective crates.

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
the same order their entries appear in it. A reader locates any chunk
by id via the TOC and must not assume a positional layout.

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

Chunk ids are 4 bytes of arbitrary content. There is no provision for
duplicate ids: a producer must not emit the same id twice.

---

## Chunk Ids

The format reserves four ids for named slots and one id namespace for
indexed slots. All slots are optional except `PRIM`.

    Id        Slot                       Required?
    ────────  ─────────────────────────  ──────────
    "SUMR"    Typed summary              No
    "META"    Opaque metadata            No
    "FLDS"    Opaque fields table        No
    "PRIM"    Primary FST                Yes
    "HC{i}"   Indexed secondary chunk    No

`"HC{i}"` denotes a chunk whose id is the 4 bytes `[b'H', b'C', hi, lo]`
where `hi:lo` is the chunk's u16 index. Indices start at 0 and are
contiguous; a producer that emits `N` secondary chunks uses ids
`HC{0}` through `HC{N-1}`.

The container does not assign meaning to `META`, `FLDS`, or any `HC{i}`
payload. Callers serialize their own types into these slots and
deserialize them on read; the container only stores and retrieves the
bytes.

`SUMR` is the one slot whose payload type is owned by this crate
(see [§ SUMR Payload](#sumr-payload)).

`PRIM`'s payload must deserialize as `fst_index::FstIndex<T>` for a
caller-chosen `T`. A file without `PRIM` is malformed; the writer
fails to produce one and the reader exposes a typed `primary()`
accessor.

---

## SUMR Payload

When `SUMR` is present, its payload bytes decompress and deserialize as:

    pub struct FileSummary {
        pub min_timestamp_s: u32,
        pub max_timestamp_s: u32,
        pub total_logs:      u32,
        pub stream:          StreamEntry,
    }

    pub struct StreamEntry {
        pub namespace: String,
        pub name:      String,
    }

Field semantics are the producer's responsibility; the format only
guarantees the shape on the wire. A reader that decodes `SUMR` gets
this struct back.

---

## Encoding

The container itself imposes no encoding on chunk payloads — they are
opaque byte sequences indexed by the TOC.

This crate's `pack` / `pack_metadata` helpers, and the `unpack` /
`unpack_metadata` inverses, implement the canonical encoding used by
existing producers and consumers:

    serialized = bincode::serde::encode(value, bincode::config::standard())
    payload    = zstd::encode(serialized, level)

The zstd compression level is a caller parameter; the format does not
fix it.

There is no container-level integrity check. The zstd frame format
includes a content checksum that catches corruption within an
individual chunk's payload, but the header and TOC are unprotected.

---

## Format Version

The current version is **2**.

A bump is required for any change that breaks the on-disk contract:

- adding a required chunk id,
- removing an existing chunk id,
- changing the `FileSummary` schema in a non-backwards-compatible way,
- changing how the TOC is laid out,
- redefining what `PRIM` decodes as.

Adding a new optional chunk id, or extending `FileSummary` with a
field whose default decodes from absent bytes, does not require a
bump.
