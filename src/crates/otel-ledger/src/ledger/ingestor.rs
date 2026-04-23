//! WAL message handling.
//!
//! The ingestor is a separate process that writes OTEL logs into per-tenant
//! WAL files and streams [`wal::FileEvent`]s back over IPC — one per
//! Created / Synced / Closed transition. This handler is the sole sink
//! for those events.
//!
//! Each message:
//! - checks `msg.frame_seq` against `expected_frame_seq` and logs a gap if
//!   IPC frames were dropped;
//! - applies the event to the owning tenant's registry — lazily creating
//!   the tenant's `Registry` on first sight, and recording the
//!   `FileId.seq`→tenant routing so later worker responses (indexer,
//!   uploader, cleaner, catalog builder) can be dispatched back to the
//!   right tenant;
//! - on `Closed`, sends an `Index` request to the indexer — kicking off
//!   the downstream indexing → upload → catalog pipeline driven by
//!   [`super::responses`].

use crate::ipc::IndexerRequest;

use super::Ledger;

impl Ledger {
    #[tracing::instrument(
        skip_all,
        fields(tenant = %msg.tenant_id, frame_seq = msg.frame_seq, event = ?msg.event),
    )]
    pub(super) async fn handle_ingestor_msg(&mut self, msg: wal::Message) {
        // Check consistency of frame sequence numbers
        if msg.frame_seq != self.expected_frame_seq {
            tracing::error!(
                "ingestor frame gap: expected={} missed={}",
                self.expected_frame_seq,
                msg.frame_seq - self.expected_frame_seq,
            );
        }
        self.expected_frame_seq = msg.frame_seq + 1;

        // Apply the event to the proper registry
        if let Err(e) = self.registries.apply_wal_event(&msg.tenant_id, &msg.event) {
            tracing::error!("failed to apply WAL event: {e}");
            return;
        }

        // Send an indexing request when a WAL file is closed.
        if let wal::FileEvent::Closed { file_id, .. } = msg.event {
            let registry = self
                .registries
                .get(&msg.tenant_id)
                .expect("tenant registry present after applying WAL event");

            let req = IndexerRequest::Index {
                wal_path: registry.wal.file_path(file_id),
                sfst_path: registry.sfst.file_path(file_id),
            };

            if let Err(e) = self.indexer.send(req) {
                tracing::error!("failed to send to indexer: {e}");
            }
        }
    }
}
