//! The query result (`RRDR`, `src/web/api/queries/rrdr.h`, `rrdr.c`): one row per point, one column per metric (v1)
//! or per group (v2). Spec §4.2.

use crate::window::Window;

/// `RRDR_VALUE_FLAGS`: a cell's flags (`r->o`).
pub mod value_flags {
    pub const NOTHING: u32 = 0;
    pub const EMPTY: u32 = 1 << 0;
    pub const RESET: u32 = 1 << 1;
    pub const PARTIAL: u32 = 1 << 2;
}

/// `RRDR_RESULT_FLAGS` (`r->view.flags`).
pub mod result_flags {
    pub const ABSOLUTE: u32 = 1 << 0;
    pub const RELATIVE: u32 = 1 << 1;
    pub const CANCEL: u32 = 1 << 2;
}

/// `r->view`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub after: i64,
    pub before: i64,
    pub group: i64,
    pub update_every: i64,
    pub min: f64,
    pub max: f64,
    pub flags: u32,
}

#[derive(Debug, Clone)]
pub struct Rrdr {
    /// Rows (`r->n`, and `r->rows` once the timestamps are set).
    pub rows: usize,
    /// Columns (`r->d`).
    pub columns: usize,
    /// Row end times, oldest first.
    pub t: Vec<i64>,
    /// Values, flags and anomaly rates, row-major (`row * columns + column`).
    pub v: Vec<f64>,
    pub o: Vec<u32>,
    pub ar: Vec<f64>,
    /// Column flags (`RRDR_DIMENSION_*`).
    pub od: Vec<u32>,
    /// Column ids and names (`r->di`, `r->dn`).
    pub di: Vec<String>,
    pub dn: Vec<String>,
    pub view: View,
    /// Metrics executed into this result (`r->internal.queries_count`); the first row of the first one seeds
    /// `view.min`/`view.max`.
    pub queries_count: usize,
    pub result_points_generated: usize,
    pub db_points_read: usize,
}

impl Rrdr {
    /// `rrdr_create()` then `rrd2rrdr_set_timestamps()`. C leaves the cells uninitialised; a column that is never
    /// executed (a failed metric) is never read, so zeroes stand in for them.
    pub fn new(window: &Window, columns: usize) -> Self {
        let rows = usize::try_from(window.points).unwrap_or(usize::MAX);
        let cells = rows * columns;
        Rrdr {
            rows,
            columns,
            t: (0..window.points).map(|i| window.row_time(i)).collect(),
            v: vec![0.0; cells],
            o: vec![value_flags::NOTHING; cells],
            ar: vec![0.0; cells],
            od: vec![0; columns],
            di: vec![String::new(); columns],
            dn: vec![String::new(); columns],
            view: View {
                after: window.after,
                before: window.before,
                group: window.group,
                update_every: window.view_update_every(),
                min: 0.0,
                max: 0.0,
                flags: 0,
            },
            queries_count: 0,
            result_points_generated: 0,
            db_points_read: 0,
        }
    }

    pub fn index(&self, row: usize, column: usize) -> usize {
        row * self.columns + column
    }
}
