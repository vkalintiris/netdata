use bridge::LedgerRequest;

use crate::ipc::{CatalogWriterResponse, CleanerResponse, IndexerResponse, UploaderResponse};

/// A unified event from any of the ledger's input sources.
pub enum LedgerEvent {
    /// A WAL message from the ingestor.
    WalMsg(wal::Message),
    /// A response from the indexer subprocess.
    IndexerResp(IndexerResponse),
    /// A response from the cleaner subprocess.
    CleanerResp(CleanerResponse),
    /// A response from the uploader subprocess.
    UploaderResp(UploaderResponse),
    /// A response from the catalog writer.
    CatalogWriterResp(CatalogWriterResponse),
    /// A request from the supervisor.
    SupervisorReq(LedgerRequest),
}
