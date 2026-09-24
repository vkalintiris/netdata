//! The data query engine, ported from `src/web/api/queries/`, `src/web/api/formatters/`,
//! `src/database/contexts/query_target.c` and the data handlers of `src/web/api/v1/` and `src/web/api/v2/`.
//! Spec: `knowledge/spec-query.md` in the status repository (decisions D22).

#![forbid(unsafe_code)]

pub mod grouping;
pub mod id;
pub mod request;
pub mod tables;
pub mod target;
pub mod window;
