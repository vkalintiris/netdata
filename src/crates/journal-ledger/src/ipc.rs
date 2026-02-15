//! IPC types for communication between components and the journal ledger.
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
    FinalizeIndex { path: PathBuf },
}

/// Responses sent from the indexer back to the ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexerResponse {
    /// The request was accepted and will be processed.
    Accepted,
    /// The index for a file has been finalized successfully.
    IndexFinalized { path: PathBuf },
    /// Indexing failed for a file.
    IndexFailed { path: PathBuf, error: String },
}

/// Listens for WAL events on a given socket path.
pub struct WalListener {
    listener: Listener<(), wal::format::WalMessage>,
}

impl WalListener {
    pub fn new(socket_path: &str) -> Result<Self, ferryboat::Error> {
        let _ = std::fs::remove_file(socket_path);
        let endpoint = Endpoint::ipc(socket_path);
        let listener = Listener::<(), wal::format::WalMessage>::bind(endpoint).open()?;
        Ok(Self { listener })
    }

    pub async fn accept(&mut self) -> Result<WalReceiver, ferryboat::Error> {
        let conn = self.listener.accept().await?;
        Ok(WalReceiver { conn })
    }
}

/// A connection from the ingestor, used to receive WAL events.
pub struct WalReceiver {
    conn: Connection<(), wal::format::WalMessage>,
}

impl WalReceiver {
    pub async fn receive(&mut self) -> Result<wal::format::WalMessage, ferryboat::Error> {
        self.conn.recv().await
    }
}
