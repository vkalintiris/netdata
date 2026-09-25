//! Rows: a newest-first page with content-key cursors, or the slowest K.
//!
//! A row's key is its content (start, trace id, span id), not its position in a
//! file, so a page walk stays exact when rows move from the live tail into a
//! chunk image or a WAL seals between pages. Rows with an identical key (a resent
//! span) are never split across pages.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use super::query::{NAME_FIELD, SERVICE_FIELD, STATUS_FIELD};

/// Most rows one newest page may hold.
pub const ROWS_PAGE_MAX: usize = 1000;
/// Most rows a slowest request may ask for.
pub const TOP_K_MAX: usize = 1000;
/// Most extra columns one request may ask for.
pub const ROW_COLUMNS_MAX: usize = 32;

/// Prefixes of the fields a span's events and links are stored under: they
/// describe many events or links at once, never the span row itself.
pub const NOT_ROW_COLUMN_PREFIXES: [&str; 2] = ["events.", "links."];

/// Whether `field` may be asked for as a row column.
pub fn is_row_column(field: &str) -> bool {
    !field.is_empty()
        && !NOT_ROW_COLUMN_PREFIXES
            .iter()
            .any(|prefix| field.starts_with(prefix))
}

/// Content key of a stored row. Newest order is the key descending; ids
/// compare as bytes, which is lowercase-hex order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowKey {
    pub start_ns: i64,
    pub trace_id: sfst::TraceId,
    pub span_id: sfst::SpanId,
}

impl RowKey {
    /// `{start_ns}:{trace id hex}:{span id hex}`.
    pub fn encode(&self) -> String {
        format!("{}:{}:{}", self.start_ns, self.trace_id, self.span_id)
    }

    /// The inverse of [`encode`](Self::encode); `None` for anything else.
    pub fn decode(cursor: &str) -> Option<RowKey> {
        let mut parts = cursor.split(':');
        let start_ns = parts.next()?.parse().ok()?;
        let trace_id = sfst::TraceId::from_bytes(&hex(parts.next()?)?)?;
        let span_id = sfst::SpanId::from_bytes(&hex(parts.next()?)?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(RowKey {
            start_ns,
            trace_id,
            span_id,
        })
    }
}

fn hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    for i in (0..text.len()).step_by(2) {
        out.push(u8::from_str_radix(&text[i..i + 2], 16).ok()?);
    }
    Some(out)
}

/// Which way a newest page walks from its anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowDirection {
    /// Rows with keys below the anchor: the next page down the list.
    Older,
    /// Rows with keys above the anchor: the page before it.
    Newer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowOrder {
    /// Newest first, a page at a time; the newest page when `anchor` is `None`
    /// and `direction` is `Older`.
    Newest {
        anchor: Option<RowKey>,
        direction: RowDirection,
    },
    /// The slowest rows: duration descending, then start descending, then
    /// trace id and span id ascending.
    Slowest,
}

/// The rows section of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowsSpec {
    pub order: RowOrder,
    /// Rows per page, or K for the slowest order.
    pub limit: usize,
    /// Extra fields per row, in storage names; every value of a multi-valued
    /// field is kept.
    pub columns: Vec<String>,
}

/// A row that may make the page, and where to read its fields.
#[derive(Debug, Clone, Copy)]
pub(super) struct Candidate {
    pub key: RowKey,
    pub duration_ns: i64,
    /// The opened source holding the row (an index the caller assigns).
    pub source: usize,
    pub position: u32,
}

impl Candidate {
    /// `Less` when `self` is slower, i.e. comes first in the slowest order.
    fn slowest_cmp(&self, other: &Candidate) -> Ordering {
        let rank = |c: &Candidate| {
            (
                Reverse(c.duration_ns),
                Reverse(c.key.start_ns),
                c.key.trace_id,
                c.key.span_id,
            )
        };
        rank(self).cmp(&rank(other))
    }
}

/// A candidate ranked for a bounded max-heap: the top is the one that would
/// leave the slowest set first.
struct Slowest(Candidate);

impl PartialEq for Slowest {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Slowest {}

impl PartialOrd for Slowest {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Slowest {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.slowest_cmp(&other.0)
    }
}

/// One source's candidates for the page and what it counted beyond them.
#[derive(Debug, Default)]
pub(super) struct SourceRows {
    pub candidates: Vec<Candidate>,
    /// Rows on the page's side of the anchor that cannot make the page.
    pub beyond: u64,
    /// Rows on the anchor's other side, the anchor's own key included.
    pub behind: u64,
}

/// Whether rows exist beyond a newest page on each side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoreRows {
    pub older: bool,
    pub newer: bool,
}

/// Candidates in the order the page is taken: newest first, oldest first when
/// walking newer, slowest first.
fn page_cmp(order: &RowOrder, a: &Candidate, b: &Candidate) -> Ordering {
    match order {
        RowOrder::Newest {
            direction: RowDirection::Newer,
            ..
        } => a.key.cmp(&b.key),
        RowOrder::Newest { .. } => b.key.cmp(&a.key),
        RowOrder::Slowest => a.slowest_cmp(b),
    }
}

/// Sort `candidates` into page order and keep `limit` of them, plus, for a
/// newest page, every candidate whose key equals the last one kept. Returns
/// how many were dropped.
fn keep_page(spec: &RowsSpec, candidates: &mut Vec<Candidate>) -> u64 {
    candidates.sort_by(|a, b| page_cmp(&spec.order, a, b));
    if candidates.len() <= spec.limit {
        return 0;
    }
    let mut end = spec.limit;
    if matches!(spec.order, RowOrder::Newest { .. }) && end > 0 {
        let last = candidates[end - 1].key;
        while end < candidates.len() && candidates[end].key == last {
            end += 1;
        }
    }
    let dropped = (candidates.len() - end) as u64;
    candidates.truncate(end);
    dropped
}

/// One source's scope rows in `window`, cut to those that can make the page.
/// `source` is stamped on every candidate.
pub(super) fn source_rows(
    reader: &sfst::IndexReader<'_>,
    scope: &sfst::BitmapFilter,
    window: std::ops::Range<i64>,
    spec: &RowsSpec,
    source: usize,
) -> Result<SourceRows, sfst::Error> {
    let positions = reader.matched_positions(scope, window)?;
    if positions.is_empty() {
        return Ok(SourceRows::default());
    }
    let timestamps = reader.load_timestamps()?;
    let trace_ids = reader.trace_ids()?;
    let span_ids = reader.span_ids()?;
    let durations = reader.durations()?;
    let rows = reader.summary().record_count as usize;
    if trace_ids.len() < rows || span_ids.len() < rows || durations.0.len() < rows {
        return Err(sfst::Error::CorruptIndex(format!(
            "{rows} rows but {} trace ids, {} span ids, {} durations",
            trace_ids.len(),
            span_ids.len(),
            durations.0.len()
        )));
    }
    let candidate = |position: u32| -> Result<Candidate, sfst::Error> {
        let index = position as usize;
        let start_ns = timestamps
            .at(position)
            .ok_or_else(|| sfst::Error::CorruptIndex(format!("no timestamp for row {position}")))?;
        Ok(Candidate {
            key: RowKey {
                start_ns,
                trace_id: trace_ids.get(index),
                span_id: span_ids.get(index),
            },
            duration_ns: durations.0[index],
            source,
            position,
        })
    };

    let mut out = SourceRows::default();
    match spec.order {
        RowOrder::Newest { anchor, direction } => {
            // Rows are stored by start time, so walking positions from the page's
            // end visits keys in page order up to ties in start time. Once the page
            // is full, the rest of the boundary row's start group is still taken
            // (the sort below settles its order); every later row is worse.
            let n = positions.len();
            let mut boundary: Option<i64> = None;
            let mut previous: Option<i64> = None;
            for step in 0..n {
                let position = match direction {
                    RowDirection::Older => positions[n - 1 - step],
                    RowDirection::Newer => positions[step],
                };
                let row = candidate(position)?;
                let start = row.key.start_ns;
                if let Some(previous) = previous {
                    let in_order = match direction {
                        RowDirection::Older => start <= previous,
                        RowDirection::Newer => start >= previous,
                    };
                    if !in_order {
                        return Err(sfst::Error::CorruptIndex(format!(
                            "row {position} is out of start-time order"
                        )));
                    }
                }
                previous = Some(start);
                if boundary.is_some_and(|b| b != start) {
                    out.beyond += (n - step) as u64;
                    break;
                }
                let on_page_side = match (anchor, direction) {
                    (None, _) => true,
                    (Some(anchor), RowDirection::Older) => row.key < anchor,
                    (Some(anchor), RowDirection::Newer) => row.key > anchor,
                };
                if !on_page_side {
                    out.behind += 1;
                    continue;
                }
                out.candidates.push(row);
                if out.candidates.len() == spec.limit {
                    boundary = Some(start);
                }
            }
            out.beyond += keep_page(spec, &mut out.candidates);
        }
        RowOrder::Slowest => {
            let mut heap = BinaryHeap::with_capacity(spec.limit + 1);
            for &position in &positions {
                let row = Slowest(candidate(position)?);
                if heap.len() == spec.limit
                    && heap.peek().is_some_and(|worst: &Slowest| row >= *worst)
                {
                    continue;
                }
                heap.push(row);
                if heap.len() > spec.limit {
                    heap.pop();
                }
            }
            for Slowest(row) in heap {
                out.candidates.push(row);
            }
            keep_page(spec, &mut out.candidates);
        }
    }
    Ok(out)
}

/// The page from every source's candidates, newest (or slowest) first, and
/// for a newest page whether rows exist beyond it on each side.
pub(super) fn select_page(
    spec: &RowsSpec,
    per_source: Vec<SourceRows>,
) -> (Vec<Candidate>, Option<MoreRows>) {
    let mut candidates = Vec::new();
    let mut beyond = 0;
    let mut behind = 0;
    for rows in per_source {
        candidates.extend(rows.candidates);
        beyond += rows.beyond;
        behind += rows.behind;
    }
    beyond += keep_page(spec, &mut candidates);
    match spec.order {
        RowOrder::Newest {
            direction: RowDirection::Older,
            ..
        } => (
            candidates,
            Some(MoreRows {
                older: beyond > 0,
                newer: behind > 0,
            }),
        ),
        RowOrder::Newest {
            direction: RowDirection::Newer,
            ..
        } => {
            candidates.reverse();
            (
                candidates,
                Some(MoreRows {
                    older: behind > 0,
                    newer: beyond > 0,
                }),
            )
        }
        RowOrder::Slowest => (candidates, None),
    }
}

/// The fields a row shows besides its key and duration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RowFields {
    pub service: Option<String>,
    pub name: Option<String>,
    pub role: Option<String>,
    pub status: Option<String>,
    /// Parallel to the request's columns; values sorted.
    pub columns: Vec<Vec<String>>,
}

/// Fields every row shows, in [`RowFields`] order.
const SHOWN_FIELDS: [&str; 4] = [
    SERVICE_FIELD,
    NAME_FIELD,
    ng_flatten::ROLE_FIELD,
    STATUS_FIELD,
];

/// Read the shown fields and `columns` of the rows at `positions` of one
/// source, in `positions` order.
pub(super) fn materialize(
    bytes: &[u8],
    positions: &[u32],
    columns: &[String],
) -> Result<Vec<RowFields>, sfst::Error> {
    let reader = sfst::IndexReader::open(bytes)?;
    let mut fields: Vec<&str> = SHOWN_FIELDS.to_vec();
    for column in columns {
        fields.push(column);
    }
    let mut values = Vec::with_capacity(fields.len());
    for field in reader.materialize_fields(&fields, positions)? {
        values.push(field.into_iter());
    }
    let mut out = Vec::with_capacity(positions.len());
    for _ in positions {
        let mut take = |field: usize| values[field].next().unwrap_or_default();
        let mut row = RowFields {
            service: take(0).into_iter().next(),
            name: take(1).into_iter().next(),
            role: take(2).into_iter().next(),
            status: take(3).into_iter().next(),
            columns: Vec::with_capacity(columns.len()),
        };
        for field in SHOWN_FIELDS.len()..fields.len() {
            let mut column = take(field);
            column.sort();
            row.columns.push(column);
        }
        out.push(row);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(start_ns: i64, trace: u8, span: u8) -> RowKey {
        RowKey {
            start_ns,
            trace_id: sfst::TraceId::from([trace; 16]),
            span_id: sfst::SpanId::from([span; 8]),
        }
    }

    fn candidates(source: usize, keys: &[(RowKey, i64)]) -> SourceRows {
        let mut rows = SourceRows::default();
        for (position, (key, duration_ns)) in keys.iter().enumerate() {
            rows.candidates.push(Candidate {
                key: *key,
                duration_ns: *duration_ns,
                source,
                position: position as u32,
            });
        }
        rows
    }

    fn newest(limit: usize, anchor: Option<RowKey>, direction: RowDirection) -> RowsSpec {
        RowsSpec {
            order: RowOrder::Newest { anchor, direction },
            limit,
            columns: Vec::new(),
        }
    }

    fn keys(page: &[Candidate]) -> Vec<RowKey> {
        page.iter().map(|c| c.key).collect()
    }

    #[test]
    fn row_keys_round_trip_and_reject_anything_else() {
        let key = RowKey {
            start_ns: 1_758_791_650_123_456_789,
            trace_id: sfst::TraceId::from([0x4b; 16]),
            span_id: sfst::SpanId::from([0x0f; 8]),
        };
        assert_eq!(
            key.encode(),
            "1758791650123456789:4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b:0f0f0f0f0f0f0f0f"
        );
        assert_eq!(RowKey::decode(&key.encode()), Some(key));
        let extra = format!("{}:00", key.encode());
        for bad in [
            "",
            "1:2:3",
            "x:4b:0f",
            "1:4b4b:0f0f",
            "1:zz:0f",
            extra.as_str(),
        ] {
            assert_eq!(RowKey::decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_group_of_identical_keys_is_never_split() {
        let resent = key(50, 1, 1);
        let spec = newest(2, None, RowDirection::Older);
        let per_source = vec![
            candidates(0, &[(key(60, 1, 1), 0), (resent, 0)]),
            candidates(1, &[(resent, 0), (key(40, 1, 1), 0)]),
        ];
        let (page, more) = select_page(&spec, per_source);
        assert_eq!(keys(&page), [key(60, 1, 1), resent, resent]);
        assert_eq!(
            more,
            Some(MoreRows {
                older: true,
                newer: false
            })
        );
    }

    #[test]
    fn a_newer_page_is_returned_newest_first() {
        let anchor = key(20, 1, 1);
        let spec = newest(2, Some(anchor), RowDirection::Newer);
        let mut rows = candidates(
            0,
            &[(key(30, 1, 1), 0), (key(40, 1, 1), 0), (key(50, 1, 1), 0)],
        );
        rows.behind = 1;
        let (page, more) = select_page(&spec, vec![rows]);
        assert_eq!(keys(&page), [key(40, 1, 1), key(30, 1, 1)]);
        assert_eq!(
            more,
            Some(MoreRows {
                older: true,
                newer: true
            })
        );
    }

    #[test]
    fn slowest_breaks_ties_by_start_then_ids() {
        let spec = RowsSpec {
            order: RowOrder::Slowest,
            limit: 3,
            columns: Vec::new(),
        };
        let per_source = vec![
            candidates(0, &[(key(10, 2, 1), 500), (key(10, 1, 2), 500)]),
            candidates(
                1,
                &[(key(20, 9, 9), 500), (key(5, 1, 1), 900), (key(1, 1, 1), 1)],
            ),
        ];
        let (page, more) = select_page(&spec, per_source);
        assert_eq!(
            keys(&page),
            [key(5, 1, 1), key(20, 9, 9), key(10, 1, 2)],
            "900 first; then 500 by start descending, then trace id ascending"
        );
        assert_eq!(more, None);
    }
}
