//! Test support for this crate's and other crates' tests (the `testing` feature).

use crate::reply::{Payload, Reply};

/// A handler for registrations no test calls.
pub fn inert(_: &mut Reply, _: &[u8], _: Option<&Payload>, _: &[u8]) -> u16 {
    200
}
