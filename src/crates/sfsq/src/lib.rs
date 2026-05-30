//! Query layer over SFST log indexes.
//!
//! [`logs`] is the multi-file log-query engine: given a set of
//! overlapping SFST files and a request, it produces a single query
//! response — facets, a histogram, and a paginated, materialized page
//! of log rows. See [`logs::run`] for the entry point.

pub mod logs;
