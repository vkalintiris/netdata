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
    /// The lifecycle calls, in order: `("receiver_left", reason)`, `("parents_reset", reason)`, `("free", 0)`,
    /// `("reinit", 0)`.
    pub calls: Mutex<Vec<(&'static str, i32)>>,
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

    fn receiver_left(&self, reason: i32) {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).push(("receiver_left", reason));
    }

    fn parents_reset(&self, reason: i32) {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).push(("parents_reset", reason));
    }

    fn free(&self) {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).push(("free", 0));
    }

    fn reinit(&self, _send: &crate::host::StreamSend) {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).push(("reinit", 0));
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
