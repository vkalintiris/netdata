#![allow(dead_code, unused_variables)]

use chrono::Utc;
use prost::Message;
use std::fs::OpenOptions;
use std::io::{self, Read};
use thiserror::Error;
use tracing::{error, info, Level};
use tracing_subscriber::fmt;

pub mod netdata {
    include!(concat!(env!("OUT_DIR"), "/netdata.rs"));
}

use netdata::{ChartDefinition, Host};

extern crate static_assertions as sa;

#[derive(Error, Debug)]
pub enum NetdataError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Failed to decode protobuf message: {0}")]
    ProtobufDecode(#[from] prost::DecodeError),

    #[error("Failed to initialize logging: {0}")]
    LogInit(String),
}

struct MessageIterator<R: Read> {
    reader: R,
    buffer: Vec<u8>,
}

impl<R: Read> MessageIterator<R> {
    fn new(reader: R) -> Result<Self, NetdataError> {
        Ok(MessageIterator {
            reader,
            buffer: Vec::new(),
        })
    }
}

impl<R: Read> Iterator for MessageIterator<R> {
    type Item = Result<Host, NetdataError>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut length_bytes = [0u8; 4];
        match self.reader.read_exact(&mut length_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return None;
            }
            Err(e) => {
                return Some(Err(NetdataError::Io(e)));
            }
        }

        let message_length = u32::from_le_bytes(length_bytes) as usize;
        if self.buffer.len() < message_length {
            self.buffer.resize(message_length, 0);
        }

        match self.reader.read_exact(&mut self.buffer[..message_length]) {
            Ok(_) => match Host::decode(&self.buffer[..message_length]) {
                Ok(mut host) => {
                    host.chart_definition.sort_by_key(|def| def.id);
                    host.chart_collection.sort_by_key(|col| col.id);

                    Some(Ok(host))
                }
                Err(e) => Some(Err(NetdataError::ProtobufDecode(e))),
            },
            Err(e) => Some(Err(NetdataError::Io(e))),
        }
    }
}

fn init_logging() -> Result<(), NetdataError> {
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("/tmp/rs.log")
        .map_err(|e| NetdataError::LogInit(format!("Failed to open log file: {}", e)))?;

    fmt()
        .with_writer(std::sync::Mutex::new(log_file))
        .with_ansi(false)
        .with_thread_ids(true)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_level(true)
        .with_timer(fmt::time::UtcTime::rfc_3339())
        .with_max_level(Level::INFO)
        .init();

    Ok(())
}

#[derive(Debug)]
struct CollectionChart {
    id: std::num::NonZeroU32,
}

sa::const_assert_eq!(
    std::mem::size_of::<CollectionChart>(),
    std::mem::size_of::<Option<CollectionChart>>()
);

#[derive(Debug)]
struct CollectionHeader<const T: usize> {
    collection_charts: [Option<CollectionChart>; T],
}

#[derive(Debug)]
struct CollectionTable<const T: usize> {
    header: CollectionHeader<T>,
}

#[derive(Debug)]
struct Collector<const T: usize> {
    collection_table: Vec<CollectionTable<T>>,
}

impl<const T: usize> Collector<T> {
    fn new() -> Self {
        Self {
            collection_table: Vec::new(),
        }
    }

    fn add_chart_definitions(&mut self, chart_definitions: &[ChartDefinition]) {
        let iter = chart_definitions.iter();

        let last_ct = self.collection_table.last();
    }
}

#[derive(Default)]
struct HostState {
    prev_chart_definitions: Vec<netdata::ChartDefinition>,
    prev_chart_collections: Vec<netdata::ChartCollection>,
}

impl HostState {
    fn compute_diffs(
        &mut self,
        current_definitions: &[netdata::ChartDefinition],
        current_collections: &[netdata::ChartCollection],
    ) {
        // Create sets for efficient comparison
        let prev_def_ids: std::collections::HashSet<_> = self
            .prev_chart_definitions
            .iter()
            .map(|def| &def.id)
            .collect();
        let current_def_ids: std::collections::HashSet<_> =
            current_definitions.iter().map(|def| &def.id).collect();

        let prev_col_ids: std::collections::HashSet<_> = self
            .prev_chart_collections
            .iter()
            .map(|col| &col.id)
            .collect();
        let current_col_ids: std::collections::HashSet<_> =
            current_collections.iter().map(|col| &col.id).collect();

        // Find added and removed definitions
        let mut added_defs: Vec<_> = current_def_ids.difference(&prev_def_ids).collect();
        let mut removed_defs: Vec<_> = prev_def_ids.difference(&current_def_ids).collect();

        added_defs.sort();
        removed_defs.sort();

        // Find added and removed collections
        let added_cols: Vec<_> = current_col_ids.difference(&prev_col_ids).collect();
        let removed_cols: Vec<_> = prev_col_ids.difference(&current_col_ids).collect();

        // Log the differences
        if !added_defs.is_empty() || !removed_defs.is_empty() {
            info!(
                added_definitions = ?added_defs,
                removed_definitions = ?removed_defs,
                "chart definition changes"
            );
        }

        // if !added_cols.is_empty() || !removed_cols.is_empty() {
        //     info!(
        //         added_collections = ?added_cols,
        //         removed_collections = ?removed_cols,
        //         "chart collection changes"
        //     );
        // }

        // Update state for next comparison
        self.prev_chart_definitions = current_definitions.to_vec();
        self.prev_chart_collections = current_collections.to_vec();
    }
}

/// Error types for buffer operations
#[derive(Debug)]
pub enum BufferError {
    /// Buffer space exhausted
    NoSpace,
    /// Invalid buffer size (must be multiple of 4)
    InvalidBufferSize,
    /// Too many series for buffer size
    TooManySeries,
    /// Attempt to add sample to non-existent series
    InvalidSeriesIndex,
    /// Buffer is full (reached 1024 samples)
    BufferFull,
    /// Series IDs and initial values arrays have different lengths
    MismatchedArrays,
}

impl std::fmt::Display for BufferError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            BufferError::NoSpace => write!(f, "insufficient buffer space for encoding"),
            BufferError::InvalidBufferSize => write!(f, "buffer size must be multiple of 4"),
            BufferError::TooManySeries => write!(f, "too many series for buffer size"),
            BufferError::InvalidSeriesIndex => write!(f, "invalid series index"),
            BufferError::BufferFull => write!(f, "buffer has reached maximum samples (1024)"),
            BufferError::MismatchedArrays => {
                write!(f, "series_ids and initial_values must have same length")
            }
        }
    }
}

impl std::error::Error for BufferError {}

pub struct TabularGorillaBuffer<'a> {
    /// Raw buffer bytes
    buffer: &'a mut [u8],
}

impl<'a> TabularGorillaBuffer<'a> {
    /// Create a new buffer from raw bytes with timestamp and series information
    ///
    /// # Arguments
    /// * `buffer` - Slice of bytes, must be 4-byte aligned
    /// * `timestamp` - Base timestamp in seconds
    /// * `series_ids` - Array of series identifiers
    /// * `initial_values` - Initial values for each series
    ///
    /// # Returns
    /// * `Result<Self, BufferError>` - New buffer or error if invalid input
    pub fn new(
        buffer: &'a mut [u8],
        timestamp: u32,
        series_ids: &[u32],
        initial_values: &[u32],
    ) -> Result<Self, BufferError> {
        // Validate inputs
        if buffer.len() % 4 != 0 {
            return Err(BufferError::InvalidBufferSize);
        }
        if series_ids.len() != initial_values.len() {
            return Err(BufferError::MismatchedArrays);
        }
        if series_ids.len() > u16::MAX as usize {
            return Err(BufferError::TooManySeries);
        }

        let num_series = series_ids.len() as u16;
        let required_size = Self::required_size(num_series);
        if buffer.len() < required_size {
            return Err(BufferError::NoSpace);
        }

        // Write header information
        let mut buf = Self { buffer };

        // Write timestamp (4 bytes)
        buf.write_u32(0, timestamp);

        // Write collection iterations (2 bytes) - starts at 0
        buf.write_u16(4, 0);

        // Write number of series (2 bytes)
        buf.write_u16(6, num_series);

        // Write series IDs (4 bytes each)
        let series_ids_offset = 8;
        for (i, &id) in series_ids.iter().enumerate() {
            buf.write_u32(series_ids_offset + i * 4, id);
        }

        // Write initial values (4 bytes each)
        let initial_values_offset = series_ids_offset + series_ids.len() * 4;
        for (i, &value) in initial_values.iter().enumerate() {
            buf.write_u32(initial_values_offset + i * 4, value);
        }

        // Initialize constant flags (all true initially)
        let constant_flags_offset = initial_values_offset + initial_values.len() * 4;
        let num_flag_bytes = (num_series as usize + 7) / 8;
        for i in 0..num_flag_bytes {
            buf.buffer[constant_flags_offset + i] = 0xFF;
        }

        Ok(buf)
    }

    /// Add new samples for all series at the next timestamp
    ///
    /// # Arguments
    /// * `values` - Array of new values, must match number of series
    ///
    /// # Returns
    /// * `Result<(), BufferError>` - Success or error if buffer full/no space
    pub fn add_samples(&mut self, values: &[u32]) -> Result<(), BufferError> {
        let num_series = self.num_series() as usize;
        if values.len() != num_series {
            return Err(BufferError::MismatchedArrays);
        }

        let iterations = self.num_samples();
        if iterations >= 1024 {
            return Err(BufferError::BufferFull);
        }

        // Calculate required space for new samples
        // This is where you'd implement the Gorilla compression space calculation
        // If it doesn't fit, return NoSpace error

        // Create a temporary buffer for new compressed data
        // Compress the new values
        // If compression succeeds:
        // 1. Update constant flags if needed
        // 2. Write compressed data
        // 3. Update collection iterations

        // For now, just increment iterations to show progress
        self.write_u16(4, iterations + 1);

        Ok(())
    }

    /// Get the number of samples currently stored
    pub fn num_samples(&self) -> u16 {
        self.read_u16(4)
    }

    /// Get the number of series in the buffer
    pub fn num_series(&self) -> u16 {
        self.read_u16(6)
    }

    /// Check if a specific series has only constant values
    pub fn is_constant(&self, series_idx: u16) -> Result<bool, BufferError> {
        let num_series = self.num_series();
        if series_idx >= num_series {
            return Err(BufferError::InvalidSeriesIndex);
        }

        let constant_flags_offset = 8 + (num_series as usize * 8);
        let byte_idx = series_idx as usize / 8;
        let bit_idx = series_idx as usize % 8;

        let flag_byte = self.buffer[constant_flags_offset + byte_idx];
        Ok((flag_byte & (1 << bit_idx)) != 0)
    }

    /// Get base timestamp
    pub fn timestamp(&self) -> u32 {
        self.read_u32(0)
    }

    /// Get series IDs
    pub fn series_ids(&self) -> &[u32] {
        let num_series = self.num_series() as usize;
        let series_ids_start = 8;
        let series_ids_end = series_ids_start + (num_series * 4);

        // Safe to create slice since we validated buffer size in new()
        unsafe {
            std::slice::from_raw_parts(
                self.buffer[series_ids_start..series_ids_end].as_ptr() as *const u32,
                num_series,
            )
        }
    }

    /// Calculate the required buffer size for a given number of series
    /// Returns minimum buffer size in bytes (will be multiple of 4)
    pub fn required_size(num_series: u16) -> usize {
        let num_series = num_series as usize;

        // Calculate sizes of each section
        let header_size = 8; // timestamp(4) + iterations(2) + num_series(2)
        let series_ids_size = num_series * 4;
        let initial_values_size = num_series * 4;
        let constant_flags_size = (num_series + 7) / 8;

        // Round up constant_flags_size to nearest 4 bytes
        let constant_flags_padded = (constant_flags_size + 3) & !3;

        // Add minimum space for compressed data (implementation dependent)
        let min_compressed_space = 1024; // Example value

        header_size
            + series_ids_size
            + initial_values_size
            + constant_flags_padded
            + min_compressed_space
    }

    // Helper methods for reading/writing integers

    fn read_u32(&self, offset: usize) -> u32 {
        u32::from_le_bytes(self.buffer[offset..offset + 4].try_into().unwrap())
    }

    fn write_u32(&mut self, offset: usize, value: u32) {
        self.buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn read_u16(&self, offset: usize) -> u16 {
        u16::from_le_bytes(self.buffer[offset..offset + 2].try_into().unwrap())
    }

    fn write_u16(&mut self, offset: usize, value: u16) {
        self.buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
}

fn main() -> Result<(), NetdataError> {
    init_logging()?;

    let stdin = io::stdin().lock();
    let message_iterator = MessageIterator::new(stdin)?;
    let mut host_state = HostState::default();

    for message in message_iterator {
        match message {
            Ok(h) => {
                // let chart_definitions = h.chart_definition.len();
                // let chart_collections = h.chart_collection.len();
                // info!(
                //     chart_definitions = chart_definitions,
                //     chart_collections = chart_collections,
                //     "new host",
                // );
                let chart_definitions: Vec<_> = h
                    .chart_definition
                    .iter()
                    .filter(|cd| cd.update_every == 1)
                    .cloned()
                    .collect();

                host_state.compute_diffs(chart_definitions.as_slice(), &h.chart_collection);
            }
            Err(e) => {
                panic!("[{}] gvd error {:?}", Utc::now().to_rfc3339(), e);
            }
        }
    }

    Ok(())
}
