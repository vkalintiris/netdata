#![allow(unused_imports, dead_code)]

use std::error;
use std::fmt;

#[derive(Clone, Debug)]
pub struct Error {
    kind: ErrorKind,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind) -> Error {
        Error { kind }
    }

    pub(crate) fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    pub(crate) fn runtime<E: error::Error>(err: E) -> Error {
        Error {
            kind: ErrorKind::RuntimeError(err.to_string()),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ErrorKind {
    RuntimeError(String),
}

impl error::Error for Error {
    fn description(&self) -> &str {
        match self.kind {
            ErrorKind::RuntimeError(_) => "runtime error",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.kind {
            ErrorKind::RuntimeError(ref s) => write!(f, "{}", s),
        }
    }
}
