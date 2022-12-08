#![allow(unused_imports, dead_code)]

pub use error::{Error, ErrorKind};

pub use client::{Client, Message};

mod client;
mod error;
