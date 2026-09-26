//! The keyword tables of the data APIs: time groupings (`query-group-over-time.c`), options (`rrdr_options.c`),
//! formats (`datasource_formats.c`), group-by and aggregation (`query-group-by.c`). Spec §2.6-2.8, §6.1.

use netdata_agent_text::c::strsep_skip;
use netdata_agent_text::json::JsonWriter;

/// `RRDR_TIME_GROUPING`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TimeGrouping {
    Average = 1,
    Min = 2,
    Max = 3,
    Sum = 4,
    IncrementalSum = 5,
    TrimmedMean1 = 6,
    TrimmedMean2 = 7,
    TrimmedMean3 = 8,
    TrimmedMean = 9,
    TrimmedMean10 = 10,
    TrimmedMean15 = 11,
    TrimmedMean20 = 12,
    TrimmedMean25 = 13,
    Median = 14,
    TrimmedMedian1 = 15,
    TrimmedMedian2 = 16,
    TrimmedMedian3 = 17,
    TrimmedMedian = 18,
    TrimmedMedian10 = 19,
    TrimmedMedian15 = 20,
    TrimmedMedian20 = 21,
    TrimmedMedian25 = 22,
    Percentile25 = 23,
    Percentile50 = 24,
    Percentile75 = 25,
    Percentile80 = 26,
    Percentile90 = 27,
    Percentile = 28,
    Percentile97 = 29,
    Percentile98 = 30,
    Percentile99 = 31,
    Stddev = 32,
    Cv = 33,
    Ses = 34,
    Des = 35,
    Countif = 36,
    Extremes = 37,
    Latest = 38,
}

use TimeGrouping as G;

/// The registry in C's table order: a name's id, and the first name of an id is its echo.
pub const TIME_GROUPINGS: [(&str, TimeGrouping); 48] = [
    ("average", G::Average),
    ("avg", G::Average),
    ("mean", G::Average),
    ("trimmed-mean1", G::TrimmedMean1),
    ("trimmed-mean2", G::TrimmedMean2),
    ("trimmed-mean3", G::TrimmedMean3),
    ("trimmed-mean5", G::TrimmedMean),
    ("trimmed-mean10", G::TrimmedMean10),
    ("trimmed-mean15", G::TrimmedMean15),
    ("trimmed-mean20", G::TrimmedMean20),
    ("trimmed-mean25", G::TrimmedMean25),
    ("trimmed-mean", G::TrimmedMean),
    ("incremental_sum", G::IncrementalSum),
    ("incremental-sum", G::IncrementalSum),
    ("median", G::Median),
    ("trimmed-median1", G::TrimmedMedian1),
    ("trimmed-median2", G::TrimmedMedian2),
    ("trimmed-median3", G::TrimmedMedian3),
    ("trimmed-median5", G::TrimmedMedian),
    ("trimmed-median10", G::TrimmedMedian10),
    ("trimmed-median15", G::TrimmedMedian15),
    ("trimmed-median20", G::TrimmedMedian20),
    ("trimmed-median25", G::TrimmedMedian25),
    ("trimmed-median", G::TrimmedMedian),
    ("percentile25", G::Percentile25),
    ("percentile50", G::Percentile50),
    ("percentile75", G::Percentile75),
    ("percentile80", G::Percentile80),
    ("percentile90", G::Percentile90),
    ("percentile95", G::Percentile),
    ("percentile97", G::Percentile97),
    ("percentile98", G::Percentile98),
    ("percentile99", G::Percentile99),
    ("percentile", G::Percentile),
    ("min", G::Min),
    ("max", G::Max),
    ("sum", G::Sum),
    ("stddev", G::Stddev),
    ("cv", G::Cv),
    ("rsd", G::Cv),
    ("coefficient-of-variation", G::Cv),
    ("ses", G::Ses),
    ("ema", G::Ses),
    ("ewma", G::Ses),
    ("des", G::Des),
    ("countif", G::Countif),
    ("extremes", G::Extremes),
    ("latest", G::Latest),
];

impl TimeGrouping {
    /// `time_grouping_parse(name, AVERAGE)`: exact names; anything else is average.
    pub fn parse(name: &[u8]) -> Self {
        TIME_GROUPINGS
            .iter()
            .find(|(n, _)| n.as_bytes() == name)
            .map_or(G::Average, |&(_, g)| g)
    }

    /// `time_grouping_tostring()`.
    pub fn name(self) -> &'static str {
        TIME_GROUPINGS
            .iter()
            .find(|(_, g)| *g == self)
            .map_or("unknown", |&(n, _)| n)
    }
}

/// `RRDR_OPTIONS` bits (`src/web/api/maps/rrdr_options.h`).
pub mod options {
    pub const NONZERO: u64 = 1 << 0;
    pub const REVERSED: u64 = 1 << 1;
    pub const ABSOLUTE: u64 = 1 << 2;
    pub const DIMS_MIN2MAX: u64 = 1 << 3;
    pub const DIMS_AVERAGE: u64 = 1 << 4;
    pub const DIMS_MIN: u64 = 1 << 5;
    pub const DIMS_MAX: u64 = 1 << 6;
    pub const SECONDS: u64 = 1 << 7;
    pub const MILLISECONDS: u64 = 1 << 8;
    pub const NULL2ZERO: u64 = 1 << 9;
    pub const OBJECTSROWS: u64 = 1 << 10;
    pub const GOOGLE_JSON: u64 = 1 << 11;
    pub const JSON_WRAP: u64 = 1 << 12;
    pub const LABEL_QUOTES: u64 = 1 << 13;
    pub const PERCENTAGE: u64 = 1 << 14;
    pub const NOT_ALIGNED: u64 = 1 << 15;
    pub const DISPLAY_ABS: u64 = 1 << 16;
    pub const MATCH_IDS: u64 = 1 << 17;
    pub const MATCH_NAMES: u64 = 1 << 18;
    pub const NATURAL_POINTS: u64 = 1 << 19;
    pub const VIRTUAL_POINTS: u64 = 1 << 20;
    pub const ANOMALY_BIT: u64 = 1 << 21;
    pub const RETURN_RAW: u64 = 1 << 22;
    pub const RETURN_JWAR: u64 = 1 << 23;
    pub const SELECTED_TIER: u64 = 1 << 24;
    pub const ALL_DIMENSIONS: u64 = 1 << 25;
    pub const SHOW_DETAILS: u64 = 1 << 26;
    pub const DEBUG: u64 = 1 << 27;
    pub const MINIFY: u64 = 1 << 28;
    pub const GROUP_BY_LABELS: u64 = 1 << 29;
    pub const MINIMAL_STATS: u64 = 1 << 30;
    pub const LONG_JSON_KEYS: u64 = 1 << 31;
    pub const MCP_INFO: u64 = 1 << 32;
    pub const RFC3339: u64 = 1 << 33;
    pub const CARDINALITY_LIMIT_ALL: u64 = 1 << 34;
    /// `RRDR_OPTION_INTERNAL_AR`: never parsed or echoed.
    pub const INTERNAL_AR: u64 = 1 << 63;
}

/// `rrdr_options[]` in C's order (the first name of a bit is its echo).
pub const OPTIONS: [(&str, u64); 48] = {
    use options::*;
    [
        ("nonzero", NONZERO),
        ("flip", REVERSED),
        ("reversed", REVERSED),
        ("reverse", REVERSED),
        ("jsonwrap", JSON_WRAP),
        ("min2max", DIMS_MIN2MAX),
        ("average", DIMS_AVERAGE),
        ("min", DIMS_MIN),
        ("max", DIMS_MAX),
        ("ms", MILLISECONDS),
        ("milliseconds", MILLISECONDS),
        ("absolute", ABSOLUTE),
        ("abs", ABSOLUTE),
        ("absolute_sum", ABSOLUTE),
        ("absolute-sum", ABSOLUTE),
        ("display_absolute", DISPLAY_ABS),
        ("display-absolute", DISPLAY_ABS),
        ("seconds", SECONDS),
        ("null2zero", NULL2ZERO),
        ("objectrows", OBJECTSROWS),
        ("google_json", GOOGLE_JSON),
        ("google-json", GOOGLE_JSON),
        ("percentage", PERCENTAGE),
        ("unaligned", NOT_ALIGNED),
        ("match_ids", MATCH_IDS),
        ("match-ids", MATCH_IDS),
        ("match_names", MATCH_NAMES),
        ("match-names", MATCH_NAMES),
        ("anomaly-bit", ANOMALY_BIT),
        ("selected-tier", SELECTED_TIER),
        ("raw", RETURN_RAW),
        ("jw-anomaly-rates", RETURN_JWAR),
        ("natural-points", NATURAL_POINTS),
        ("virtual-points", VIRTUAL_POINTS),
        ("all-dimensions", ALL_DIMENSIONS),
        ("details", SHOW_DETAILS),
        ("debug", DEBUG),
        ("plan", DEBUG),
        ("minify", MINIFY),
        ("group-by-labels", GROUP_BY_LABELS),
        ("label-quotes", LABEL_QUOTES),
        ("minimal-stats", MINIMAL_STATS),
        ("minimal", MINIMAL_STATS),
        ("long-json-keys", LONG_JSON_KEYS),
        ("long-keys", LONG_JSON_KEYS),
        ("mcp-info", MCP_INFO),
        ("rfc3339", RFC3339),
        ("cardinality-limit-all", CARDINALITY_LIMIT_ALL),
    ]
};

/// The bits of the words of `o` in `table` (separators `,`, space and `|`; unknown words ignored), as
/// `rrdr_options_parse()` and `contexts_options_str_to_id()` read them.
fn parse_names(table: &[(&str, u64)], o: &[u8]) -> u64 {
    let mut bits = 0;
    let mut rest = Some(o);
    while rest.is_some_and(|r| !r.is_empty()) {
        let word = strsep_skip(&mut rest, b", |");
        if let Some(&(_, bit)) = table.iter().find(|(n, _)| n.as_bytes() == word) {
            bits |= bit;
        }
    }
    bits
}

/// The first name of each set bit, once, in table order.
fn names_of(
    table: &'static [(&'static str, u64)],
    bits: u64,
) -> impl Iterator<Item = &'static str> {
    let mut used = 0u64;
    table
        .iter()
        .filter(move |&&(_, bit)| {
            let first = bits & bit != 0 && used & bit == 0;
            used |= bit & bits;
            first
        })
        .map(|&(name, _)| name)
}

fn names_to_json_array(w: &mut JsonWriter, key: &[u8], names: impl Iterator<Item = &'static str>) {
    w.member_add_array(Some(key));
    for name in names {
        w.add_array_item_string(name);
    }
    w.array_close();
}

/// `rrdr_options_parse()`.
pub fn parse_options(o: &[u8]) -> u64 {
    parse_names(&OPTIONS, o)
}

fn option_names(bits: u64) -> impl Iterator<Item = &'static str> {
    names_of(&OPTIONS, bits)
}

/// `rrdr_options_to_buffer_json_array()`.
pub fn options_to_json_array(w: &mut JsonWriter, key: &[u8], bits: u64) {
    names_to_json_array(w, key, option_names(bits));
}

/// `CONTEXTS_OPTIONS` (`src/web/api/maps/contexts_options.h`).
pub mod contexts_options {
    pub const MINIFY: u64 = 1 << 0;
    pub const DEBUG: u64 = 1 << 1;
    pub const CONFIGURATIONS: u64 = 1 << 2;
    pub const INSTANCES: u64 = 1 << 3;
    pub const VALUES: u64 = 1 << 4;
    pub const SUMMARY: u64 = 1 << 5;
    pub const MCP: u64 = 1 << 6;
    pub const DIMENSIONS: u64 = 1 << 7;
    pub const LABELS: u64 = 1 << 8;
    pub const PRIORITIES: u64 = 1 << 9;
    pub const TITLES: u64 = 1 << 10;
    pub const RETENTION: u64 = 1 << 11;
    pub const LIVENESS: u64 = 1 << 12;
    pub const FAMILY: u64 = 1 << 13;
    pub const UNITS: u64 = 1 << 14;
    pub const RFC3339: u64 = 1 << 15;
    pub const JSON_LONG_KEYS: u64 = 1 << 16;
}

/// `contexts_options[]` in C's order (the first name of a bit is its echo).
pub const CONTEXTS_OPTIONS: [(&str, u64); 18] = {
    use contexts_options::*;
    [
        ("minify", MINIFY),
        ("debug", DEBUG),
        ("config", CONFIGURATIONS),
        ("instances", INSTANCES),
        ("values", VALUES),
        ("summary", SUMMARY),
        ("mcp", MCP),
        ("dimensions", DIMENSIONS),
        ("labels", LABELS),
        ("priorities", PRIORITIES),
        ("titles", TITLES),
        ("retention", RETENTION),
        ("liveness", LIVENESS),
        ("family", FAMILY),
        ("units", UNITS),
        ("rfc3339", RFC3339),
        ("long-json-keys", JSON_LONG_KEYS),
        ("long-keys", JSON_LONG_KEYS),
    ]
};

/// `contexts_options_str_to_id()`.
pub fn parse_contexts_options(o: &[u8]) -> u64 {
    parse_names(&CONTEXTS_OPTIONS, o)
}

/// `contexts_options_to_buffer_json_array()`.
pub fn contexts_options_to_json_array(w: &mut JsonWriter, key: &[u8], bits: u64) {
    names_to_json_array(w, key, names_of(&CONTEXTS_OPTIONS, bits));
}

/// `web_client_api_request_data_vX_options_to_string(buf, 100, options)`: comma-joined, at most 99 bytes, cut
/// wherever the limit falls.
pub fn options_to_id_string(bits: u64) -> String {
    let mut out = Vec::new();
    for (i, name) in option_names(bits).enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.extend_from_slice(name.as_bytes());
    }
    out.truncate(99);
    String::from_utf8_lossy(&out).into_owned()
}

/// `DATASOURCE_FORMAT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json = 0,
    Datatable = 1,
    Datasource = 2,
    Ssv = 3,
    Csv = 4,
    Jsonp = 5,
    Tsv = 6,
    Html = 7,
    Array = 8,
    SsvComma = 9,
    CsvJsonArray = 10,
    Markdown = 11,
    Json2 = 12,
}

/// `api_v1_data_formats[]` in C's order.
pub const FORMATS: [(&str, Format); 14] = [
    ("datatable", Format::Datatable),
    ("datasource", Format::Datasource),
    ("json", Format::Json),
    ("json2", Format::Json2),
    ("jsonp", Format::Jsonp),
    ("ssv", Format::Ssv),
    ("csv", Format::Csv),
    ("tsv", Format::Tsv),
    ("tsv-excel", Format::Tsv),
    ("html", Format::Html),
    ("array", Format::Array),
    ("ssvcomma", Format::SsvComma),
    ("csvjsonarray", Format::CsvJsonArray),
    ("markdown", Format::Markdown),
];

impl Format {
    /// `datasource_format_str_to_id()`: unknown names are `json`.
    pub fn parse(name: &[u8]) -> Self {
        FORMATS
            .iter()
            .find(|(n, _)| n.as_bytes() == name)
            .map_or(Format::Json, |&(_, f)| f)
    }

    /// `google_data_format_str_to_id()`: the `tqx=out:` map.
    pub fn from_google(name: &[u8]) -> Self {
        match name {
            b"json" => Format::Datasource,
            b"html" => Format::Html,
            b"csv" => Format::Csv,
            b"tsv-excel" => Format::Tsv,
            _ => Format::Json,
        }
    }

    /// `rrdr_format_to_string()`.
    pub fn name(self) -> &'static str {
        FORMATS
            .iter()
            .find(|(_, f)| *f == self)
            .map_or("unknown", |&(n, _)| n)
    }
}

/// `RRDR_GROUP_BY` bits.
pub mod group_by {
    pub const NONE: u32 = 0;
    pub const SELECTED: u32 = 1 << 0;
    pub const DIMENSION: u32 = 1 << 1;
    pub const INSTANCE: u32 = 1 << 2;
    pub const LABEL: u32 = 1 << 3;
    pub const NODE: u32 = 1 << 4;
    pub const CONTEXT: u32 = 1 << 5;
    pub const UNITS: u32 = 1 << 6;
    pub const PERCENTAGE_OF_INSTANCE: u32 = 1 << 7;
}

/// `group_by_parse()`: separators `,`, `|` and space; `selected` alone wins, then `percentage-of-instance`.
pub fn parse_group_by(s: &[u8]) -> u32 {
    use group_by::*;
    let mut bits = NONE;
    let mut rest = Some(s);
    while rest.is_some_and(|r| !r.is_empty()) {
        bits |= match strsep_skip(&mut rest, b", |") {
            b"selected" => SELECTED,
            b"dimension" => DIMENSION,
            b"instance" => INSTANCE,
            b"label" => LABEL,
            b"node" => NODE,
            b"context" => CONTEXT,
            b"units" => UNITS,
            b"percentage-of-instance" => PERCENTAGE_OF_INSTANCE,
            _ => NONE,
        };
    }
    if bits & SELECTED != 0 {
        SELECTED
    } else if bits & PERCENTAGE_OF_INSTANCE != 0 {
        PERCENTAGE_OF_INSTANCE
    } else {
        bits
    }
}

/// `group_by_to_buffer_json_array()` order; zero echoes `none`.
pub fn group_by_names(bits: u32) -> Vec<&'static str> {
    use group_by::*;
    if bits == NONE {
        return vec!["none"];
    }
    [
        (DIMENSION, "dimension"),
        (INSTANCE, "instance"),
        (PERCENTAGE_OF_INSTANCE, "percentage-of-instance"),
        (LABEL, "label"),
        (NODE, "node"),
        (CONTEXT, "context"),
        (UNITS, "units"),
        (SELECTED, "selected"),
    ]
    .into_iter()
    .filter(|(bit, _)| bits & bit != 0)
    .map(|(_, name)| name)
    .collect()
}

/// `RRDR_GROUP_BY_FUNCTION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregation {
    Average = 0,
    Min = 1,
    Max = 2,
    Sum = 3,
    Percentage = 4,
    Extremes = 5,
}

impl Aggregation {
    /// `group_by_aggregate_function_parse()`: anything unknown is average.
    pub fn parse(s: &[u8]) -> Self {
        match s {
            b"min" => Aggregation::Min,
            b"max" => Aggregation::Max,
            b"sum" => Aggregation::Sum,
            b"percentage" => Aggregation::Percentage,
            b"extremes" => Aggregation::Extremes,
            _ => Aggregation::Average,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Aggregation::Average => "average",
            Aggregation::Min => "min",
            Aggregation::Max => "max",
            Aggregation::Sum => "sum",
            Aggregation::Percentage => "percentage",
            Aggregation::Extremes => "extremes",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_groupings_parse_and_echo() {
        assert_eq!(TimeGrouping::parse(b"trimmed-mean"), G::TrimmedMean);
        assert_eq!(TimeGrouping::parse(b"trimmed-mean").name(), "trimmed-mean5");
        assert_eq!(TimeGrouping::parse(b"percentile").name(), "percentile95");
        assert_eq!(TimeGrouping::parse(b"ewma").name(), "ses");
        assert_eq!(
            TimeGrouping::parse(b"incremental_sum").name(),
            "incremental_sum"
        );
        assert_eq!(
            TimeGrouping::parse(b"AVG"),
            G::Average,
            "case-sensitive: unknown is average"
        );
        assert_eq!(TimeGrouping::parse(b"latest") as u8, 38);
    }

    #[test]
    fn options_parse_and_echo() {
        use options::*;
        let bits = parse_options(b"reverse|abs,unknown ms\tseconds");
        // TAB is not a separator: "ms\tseconds" is one unknown word.
        assert_eq!(bits, REVERSED | ABSOLUTE);
        let names: Vec<_> = option_names(JSON_WRAP | RETURN_JWAR | VIRTUAL_POINTS).collect();
        assert_eq!(names, ["jsonwrap", "jw-anomaly-rates", "virtual-points"]);
        let all = OPTIONS.iter().fold(0, |b, &(_, bit)| b | bit);
        let id = options_to_id_string(all);
        assert_eq!(id.len(), 99);
        assert!(id.starts_with("nonzero,flip,jsonwrap,"));
    }

    #[test]
    fn formats_group_by_and_aggregation() {
        assert_eq!(Format::parse(b"tsv-excel").name(), "tsv");
        assert_eq!(Format::parse(b"bogus"), Format::Json);
        assert_eq!(Format::from_google(b"json"), Format::Datasource);
        use group_by::*;
        assert_eq!(parse_group_by(b"node,label"), NODE | LABEL);
        assert_eq!(parse_group_by(b"node selected"), SELECTED);
        assert_eq!(
            parse_group_by(b"instance|percentage-of-instance"),
            PERCENTAGE_OF_INSTANCE
        );
        assert_eq!(
            group_by_names(NODE | DIMENSION | LABEL),
            ["dimension", "label", "node"]
        );
        assert_eq!(group_by_names(NONE), ["none"]);
        assert_eq!(Aggregation::parse(b"avg"), Aggregation::Average);
        assert_eq!(Aggregation::parse(b"extremes").name(), "extremes");
    }
}
