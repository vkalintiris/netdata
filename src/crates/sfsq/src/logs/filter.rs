//! The query's terms compiled against one index file: the field chips and
//! the text query, then the trace and span ids as global terms. Both the
//! statistics and the page compile through here, so they select the same
//! rows.

use super::query::LogsQuery;

/// The query's filter over `reader`. A file without a trace-id (span-id)
/// column holds no record of any trace (span), so an id term selects none of
/// its rows.
pub(super) fn compile(
    reader: &sfst::IndexReader<'_>,
    query: &LogsQuery,
) -> Result<sfst::BitmapFilter, sfst::Error> {
    let mut filter = reader.compile_filter(&query.filter, query.query())?;
    if !query.trace_ids.is_empty() {
        let ids = if has_column(reader, sfst::TraceIds::NAME) {
            &query.trace_ids[..]
        } else {
            &[]
        };
        filter = filter.conjoin(&reader.compile_trace_ids(ids)?);
    }
    if !query.span_ids.is_empty() {
        let ids = if has_column(reader, sfst::SpanIds::NAME) {
            &query.span_ids[..]
        } else {
            &[]
        };
        filter = filter.conjoin(&reader.compile_span_ids(ids)?);
    }
    Ok(filter)
}

pub(super) fn has_column(reader: &sfst::IndexReader<'_>, name: &str) -> bool {
    reader.columns_table().get(name).is_some()
}
