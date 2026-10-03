//! Test support for this crate's and other crates' tests (the `testing` feature).

use std::sync::{Arc, OnceLock};

use crate::{Builtin, Handler};

/// A handler for registrations no test calls; every call gives the same one, as C's one function pointer.
pub fn inert() -> Handler {
    static INERT: OnceLock<Builtin> = OnceLock::new();
    Handler::Builtin(Arc::clone(INERT.get_or_init(|| Arc::new(|_: &mut _, _: &[u8], _: Option<&_>, _: &[u8]| 200))))
}
