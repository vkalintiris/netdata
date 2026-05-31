//! Error types for SFST format operations ([`Error`]) and for building an
//! SFST file from a WAL ([`IndexError`]).

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Underlying I/O failed during read/write/flush/sync. Auto-lifted
    /// from [`std::io::Error`].
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// `bincode::encode_to_vec` rejected a value while packing a chunk
    /// payload.
    #[error("bincode encode error: {0}")]
    BincodeEncode(#[from] bincode::error::EncodeError),

    /// `bincode::decode_from_slice` failed while unpacking a chunk
    /// payload — the bytes don't match the expected shape, or are
    /// truncated.
    #[error("bincode decode error: {0}")]
    BincodeDecode(#[from] bincode::error::DecodeError),

    /// `zstd` compression or decompression failed without surfacing as
    /// [`std::io::Error`] — e.g., invalid frame header, truncated
    /// frame, checksum mismatch.
    #[error("zstd error (not std::io): {0}")]
    Zstd(String),

    /// [`Reader::open`](crate::Reader::open) found the first 4 bytes
    /// aren't `"SFST"`. The byte stream is either not an SFST file or
    /// has been corrupted before the header.
    #[error("invalid magic (expected \"SFST\")")]
    InvalidMagic,

    /// [`Reader::open`](crate::Reader::open) found a header `version`
    /// field this build of `sfst` doesn't recognize.
    #[error("unsupported version: {0}")]
    UnsupportedVersion(u32),

    /// A chunk lookup by index found no matching id — e.g.,
    /// [`Reader::mid_field`](crate::Reader::mid_field) called with an
    /// index past the file's mid-card field count.
    #[error("chunk not found: index {0}")]
    ChunkNotFound(u16),

    /// [`Writer::write_to`](crate::Writer::write_to) was called without
    /// [`Writer::set_primary`](crate::Writer::set_primary) having been
    /// called first. The `PRIM` chunk is mandatory.
    #[error("no primary chunk set")]
    NoPrimary,

    /// [`Writer::write_to`](crate::Writer::write_to) was called without
    /// [`Writer::set_timestamps`](crate::Writer::set_timestamps) having
    /// been called first. The `TIMS` chunk is mandatory.
    #[error("no timestamps chunk set")]
    NoTimestamps,

    /// [`Writer::write_to`](crate::Writer::write_to) found the number of
    /// stream-batch chunks (set via
    /// [`Writer::add_stream_batch`](crate::Writer::add_stream_batch)) is
    /// not in `1..=`[`MAX_STREAM_BATCHES`](crate::MAX_STREAM_BATCHES).
    /// Carries the actual count that was rejected.
    #[error("invalid stream-batch count: {0} (expected 1..=8)")]
    InvalidStreamBatchCount(usize),

    /// `gix-chunk` failed to parse the TOC (on open) or lay it out
    /// (on write). Carries `gix-chunk`'s own error message.
    #[error("TOC error: {0}")]
    Toc(String),

    /// The byte slice handed to [`Reader::open`](crate::Reader::open)
    /// is shorter than the 12-byte fixed header. First value is the
    /// actual length, second is the required minimum.
    #[error("file too short ({0} bytes, need at least {1})")]
    FileTooShort(usize, usize),

    /// [`IndexReader::facets`](crate::IndexReader::facets) was passed a
    /// field name that doesn't appear in this file's field table.
    /// [`matched_count`](crate::IndexReader::matched_count) /
    /// [`matched_positions`](crate::IndexReader::matched_positions) treat an
    /// absent filter field as matching no logs, and
    /// [`timeline`](crate::IndexReader::timeline) treats an absent
    /// field as "every log lacks it" (all `unset`); none return
    /// this error.
    #[error("unknown field: {0}")]
    UnknownField(String),

    /// [`IndexReader::facets`](crate::IndexReader::facets) or
    /// [`IndexReader::timeline`](crate::IndexReader::timeline) was asked
    /// to aggregate over a high-cardinality field. Per-value counts on
    /// high-card fields would require scanning stream batches, which is
    /// out of scope for the facet/timeline API.
    #[error("facet/timeline not supported for high-cardinality field: {0}")]
    HighCardFacet(String),

    /// [`IndexReader::timeline`](crate::IndexReader::timeline) was called
    /// with a non-positive bucket width.
    #[error("invalid bucket width: {0} (must be > 0)")]
    InvalidBucketWidth(i64),
}

/// Error type for the WAL → SFST indexing pipeline
/// ([`crate::index`], [`crate::build_and_write`]).
///
/// Wraps the failure modes of every layer the pipeline touches: the WAL
/// reader, the OTAP/Arrow frame decoder, FST construction, and the SFST
/// format writer (via [`Format`](IndexError::Format)).
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    /// Underlying I/O failed while reading the WAL, writing the SFST
    /// output, or renaming the temp file into place.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The SFST format layer (writer / `pack`) failed while emitting a
    /// chunk or assembling the on-disk container.
    #[error("SFST format error: {0}")]
    Format(#[from] Error),

    /// The WAL reader rejected the input — bad header, CRC mismatch,
    /// unsupported version, or a frame that failed to deserialize.
    #[error("WAL error: {0}")]
    Wal(#[from] wal::Error),

    /// Arrow IPC parsing failed while decoding an OTAP sub-stream
    /// (schema message, record batch, or column data).
    #[error("Arrow IPC error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    /// FST construction failed — almost always because the key set
    /// wasn't sortable into the FST's required lexicographic order.
    #[error("FST build error: {0}")]
    FstBuild(#[from] fst_index::BuildError),

    /// An OTAP sub-stream ran out of bytes mid-header or mid-payload:
    /// the 1-byte tag + 4-byte length prefix was incomplete, or the
    /// declared length pointed past the end of the frame.
    #[error("truncated OTAP frame")]
    TruncatedOtapFrame,

    /// An OTAP sub-stream's 1-byte tag didn't map to any known
    /// [`ArrowPayloadType`](otap_df_pdata::proto::opentelemetry::arrow::v1::ArrowPayloadType).
    /// Usually means a newer protocol version produced a payload this
    /// build doesn't recognize.
    #[error("unknown OTAP payload type tag: {0}")]
    UnknownOtapTag(i32),

    /// The WAL contains records that resolve to more than one
    /// `(service.namespace, service.name)` pair. Each SFST file is
    /// required to hold exactly one stream identity, so this fails the
    /// build. Almost always indicates an `ns_hash` collision that
    /// slipped past the ingestor's canonical-stream table, or an
    /// ingestor bug that routed mismatched writes into the same file.
    #[error(
        "WAL contains multiple stream identities (ns_hash collision or ingestor bug): \
         namespaces={namespaces:?}, names={names:?}"
    )]
    MultipleStreams {
        namespaces: Vec<String>,
        names: Vec<String>,
    },
}
