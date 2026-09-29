//! Test support for this crate's and other crates' tests (the `testing` feature).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::upstream::{Traffic, Upstream};

/// The sender as the collectors see it, recording what they commit.
#[derive(Debug, Default)]
pub struct Recorder {
    pub capabilities: AtomicU32,
    pub starts: AtomicU32,
    /// The last flush of the sender's buffer.
    pub flush_ut: AtomicU64,
    commits: Mutex<Vec<(Traffic, String)>>,
}

impl Upstream for Recorder {
    fn start(&self) {
        self.starts.fetch_add(1, Ordering::Relaxed);
    }

    fn disabled_capabilities(&self) -> u32 {
        0
    }

    fn capabilities(&self) -> u32 {
        self.capabilities.load(Ordering::Relaxed)
    }

    fn commit(&self, bytes: &[u8], traffic: Traffic) {
        // sender_buffer_commit() drops an empty message
        if !bytes.is_empty() {
            self.commits
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((traffic, String::from_utf8_lossy(bytes).into_owned()));
        }
    }

    fn resync_iterations(&self) -> u16 {
        3
    }

    fn flush_ut(&self) -> u64 {
        self.flush_ut.load(Ordering::Relaxed)
    }

    fn commit_since(&self, bytes: &[u8], traffic: Traffic, flush_ut: u64) -> bool {
        if bytes.is_empty() || flush_ut != Upstream::flush_ut(self) {
            return false;
        }
        self.commit(bytes, traffic);
        true
    }
}

impl Recorder {
    /// A sender that negotiated `capabilities`.
    pub fn with_capabilities(capabilities: u32) -> Self {
        let r = Recorder::default();
        r.capabilities.store(capabilities, Ordering::Relaxed);
        r
    }

    /// What was committed since the last take.
    pub fn take(&self) -> Vec<(Traffic, String)> {
        std::mem::take(&mut *self.commits.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
