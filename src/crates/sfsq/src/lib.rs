//! Query layer over SFST log indexes.
//!
//! [`logs`] is the multi-file log-query engine: it turns a set of
//! overlapping SFST files plus a request into the UI response envelope
//! — facets, histogram, and a paginated, materialized page of rows.
//! See [`logs::run`].

pub mod logs;
