use thiserror::Error;

#[derive(Error, Debug)]
pub enum NdError {
    /// Buffer space has been exhausted
    #[error("insufficient buffer space for encoding")]
    NoSpace,

    /// Buffer has reached maximum sample capacity
    #[error("buffer has reached maximum samples (1024)")]
    BufferFull,

    /// All available series ids are used
    #[error("Series IDs array is full")]
    SeriesIdsArrayFull,

    // Found not a valid series id
    #[error("Series ID is invalid: {0}")]
    InvalidSeriesId(u32),

    /// Duplicate series IDs found
    #[error("Found duplicate series ID: {0}")]
    DuplicateSeriesId(u32),

    /// Invalid initial values length
    #[error("Initial initial values length: {0}")]
    InvalidInitialValues(usize),
}

// /// Errors that can occur during Netdata operations
// #[derive(Error, Debug)]
// pub enum NetdataError {
//     /// IO-related errors
//     #[error("IO error: {0}")]
//     Io(#[from] std::io::Error),

//     /// Protobuf decoding errors
//     #[error("Failed to decode protobuf message: {0}")]
//     ProtobufDecode(#[from] prost::DecodeError),

//     /// Logging initialization errors
//     #[error("Failed to initialize logging: {0}")]
//     LogInit(String),

//     /// Buffer-related errors
//     #[error("Buffer error: {0}")]
//     Buffer(#[from] NetdataError),
// }
