//! IPC types for communication between components and the ledger.
//!
//! The ledger is the central coordinator. Components (otel-plugin, indexer,
//! compressor, etc.) connect to it via ferryboat over Unix domain sockets.

use std::path::PathBuf;

use ferryboat::{Connection, Endpoint, Listener};
use serde::{Deserialize, Serialize};

/// Default socket path for the writer → ledger connection.
pub const WRITER_SOCKET_PATH: &str = "/tmp/netdata-ledger-writer.sock";

/// In-process endpoint name for the indexer.
pub const INDEXER_ENDPOINT: &str = "indexer";

/// In-process endpoint name for the cleaner.
pub const CLEANER_ENDPOINT: &str = "cleaner";

/// In-process endpoint name for the uploader.
pub const UPLOADER_ENDPOINT: &str = "uploader";

/// Requests sent from the ledger to the cleaner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CleanerRequest {
    /// Delete an index file (.sfst) when retention evicts it.
    DeleteIndexFile { sequence: u64, path: PathBuf },
}

/// Responses sent from the cleaner back to the ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CleanerResponse {
    /// The request was accepted and will be processed.
    Accepted,
    /// An index file has been deleted.
    IndexFileDeleted { sequence: u64 },
    /// Failed to delete an index file.
    IndexFileFailed { sequence: u64, error: String },
}

/// Requests sent from the ledger to the indexer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexerRequest {
    /// The file has been archived — finalize its index.
    FinalizeIndex {
        /// Path to the WAL .bin file.
        wal_path: PathBuf,
        /// Path where the .sfst index should be written.
        index_path: PathBuf,
    },
}

/// Responses sent from the indexer back to the ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexerResponse {
    /// The request was accepted and will be processed.
    Accepted,
    /// The index for a file has been finalized successfully.
    IndexFinalized { seq: u64, path: PathBuf },
    /// Indexing failed for a file.
    IndexFailed { path: PathBuf, error: String },
}

/// Requests sent from the ledger to the uploader.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UploaderRequest {
    /// Upload an index file to remote object storage.
    Upload {
        seq: u64,
        local_path: PathBuf,
        remote_key: String,
    },
}

/// Responses sent from the uploader back to the ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UploaderResponse {
    /// The request was accepted and will be processed.
    Accepted,
    /// The file has been uploaded successfully.
    Uploaded { seq: u64, remote_key: String },
    /// Failed to upload the file.
    UploadFailed { seq: u64, error: String },
}

/// Accept a WAL event connection from the ingestor on the given socket path.
pub async fn accept_writer(
    socket_path: &str,
) -> Result<Connection<(), wal::format::WalMessage>, ferryboat::Error> {
    let _ = std::fs::remove_file(socket_path);
    let endpoint = Endpoint::ipc(socket_path);
    let mut listener = Listener::<(), wal::format::WalMessage>::bind(endpoint).open()?;
    listener.accept().await
}
