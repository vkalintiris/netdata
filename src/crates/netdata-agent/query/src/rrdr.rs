//! The query result (`RRDR`, `src/web/api/queries/rrdr.h`, `rrdr.c`): one row per point, one column per metric (v1)
//! or per group (v2). Spec §4.2.

use netdata_agent_storage::storage_point::StoragePoint;

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

/// `r->partial_data_trimming`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Trimming {
    pub max_update_every: i64,
    pub expected_after: i64,
    pub trimmed_after: i64,
}

/// A group's member labels (`r->dl[d]`): keys, each with its values, in first-seen order.
pub type GroupLabels = Vec<(Vec<u8>, Vec<Vec<u8>>)>;

#[derive(Debug, Clone)]
pub struct Rrdr {
    /// Rows allocated (`r->n`) and rows shown (`r->rows`, fewer after v2 live-edge trimming).
    pub n: usize,
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
    /// What the cardinality limit folded (`r->cardinality`): how many columns, and the largest folded contribution.
    pub cardinality_folded: usize,
    pub cardinality_cut: f64,
    /// The v2 group-by arrays, empty where C has NULL: per column units, priority, view statistics, metrics
    /// grouped, merged query points, slot in the next pass and member labels; per cell contributions, anomaly
    /// contributors, hidden values and hidden contributions.
    pub du: Vec<String>,
    pub dp: Vec<usize>,
    pub dview: Vec<StoragePoint>,
    pub dgbc: Vec<u32>,
    pub dqp: Vec<StoragePoint>,
    pub dgbs: Vec<usize>,
    pub dl: Option<Vec<GroupLabels>>,
    /// Every label key of the groups' members, first seen first (`r->label_keys`, with `group-by-labels`).
    pub label_keys: Option<Vec<Vec<u8>>>,
    pub gbc: Vec<u32>,
    pub arc: Vec<u32>,
    pub vh: Vec<f64>,
    pub hgbc: Vec<u32>,
    pub trimming: Trimming,
}

impl Rrdr {
    /// `rrdr_create()` then `rrd2rrdr_set_timestamps()`. C leaves the cells uninitialised; a column that is never
    /// executed (a failed metric) is never read, so zeroes stand in for them.
    pub fn new(window: &Window, columns: usize) -> Self {
        let rows = usize::try_from(window.points).unwrap_or(usize::MAX);
        let cells = rows * columns;
        Rrdr {
            n: rows,
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
            cardinality_folded: 0,
            cardinality_cut: 0.0,
            du: Vec::new(),
            dp: Vec::new(),
            dview: Vec::new(),
            dgbc: Vec::new(),
            dqp: Vec::new(),
            dgbs: Vec::new(),
            dl: None,
            label_keys: None,
            gbc: Vec::new(),
            arc: Vec::new(),
            vh: Vec::new(),
            hgbc: Vec::new(),
            trimming: Trimming::default(),
        }
    }

    pub fn index(&self, row: usize, column: usize) -> usize {
        row * self.columns + column
    }
}
