//! Storage engines of the agent: the sample encodings shared by every engine, then the engines themselves.

#![forbid(unsafe_code)]

pub mod dbengine;
pub mod ram;
pub mod storage_number;
pub mod storage_point;
