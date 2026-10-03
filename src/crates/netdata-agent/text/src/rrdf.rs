//! The columns of a Function's table, ported from `src/libnetdata/buffer/functions_fields.{h,c}`: C's field enums
//! with their names, and `buffer_rrdf_table_add_field()`'s JSON.

use crate::json::JsonWriter;

/// `RRDF_FIELD_OPTIONS`.
pub mod opts {
    pub const NONE: u8 = 0;
    /// The field is the row's unique key.
    pub const UNIQUE_KEY: u8 = 1 << 0;
    pub const VISIBLE: u8 = 1 << 1;
    pub const STICKY: u8 = 1 << 2;
    pub const FULL_WIDTH: u8 = 1 << 3;
    pub const WRAP: u8 = 1 << 4;
    /// Not a presentable field.
    pub const DUMMY: u8 = 1 << 5;
    pub const EXPANDED_FILTER: u8 = 1 << 6;
}

/// `RRDF_FIELD_SORT`: a direction and, with `FIXED`, not sortable. C names the value whole, so a direction with
/// `FIXED` prints the default, `ascending`.
pub mod sort {
    pub const ASCENDING: u8 = 1 << 0;
    pub const DESCENDING: u8 = 1 << 1;
    pub const FIXED: u8 = 1 << 7;

    /// `RRDF_FIELD_SORT_2str()`.
    pub fn name(sort: u8) -> &'static str {
        match sort {
            DESCENDING => "descending",
            _ => "ascending",
        }
    }
}

/// `RRDF_FIELD_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    None,
    Integer,
    Boolean,
    String,
    DetailString,
    BarWithInteger,
    Duration,
    Timestamp,
    Array,
}

impl FieldType {
    /// `RRDF_FIELD_TYPE_2str()`.
    pub fn name(self) -> &'static str {
        match self {
            FieldType::None => "none",
            FieldType::Integer => "integer",
            FieldType::Boolean => "boolean",
            FieldType::String => "string",
            FieldType::DetailString => "detail-string",
            FieldType::BarWithInteger => "bar-with-integer",
            FieldType::Duration => "duration",
            FieldType::Timestamp => "timestamp",
            FieldType::Array => "array",
        }
    }
}

/// `RRDF_FIELD_VISUAL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visual {
    Value,
    Bar,
    Pill,
    Rich,
    RowOptions,
}

impl Visual {
    /// `RRDF_FIELD_VISUAL_2str()`.
    pub fn name(self) -> &'static str {
        match self {
            Visual::Value => "value",
            Visual::Bar => "bar",
            Visual::Pill => "pill",
            Visual::Rich => "richValue",
            Visual::RowOptions => "rowOptions",
        }
    }
}

/// `RRDF_FIELD_TRANSFORM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transform {
    None,
    Number,
    DurationS,
    DatetimeMs,
    DatetimeUsec,
    Xml,
}

impl Transform {
    /// `RRDF_FIELD_TRANSFORM_2str()`.
    pub fn name(self) -> &'static str {
        match self {
            Transform::None => "none",
            Transform::Number => "number",
            Transform::DurationS => "duration",
            Transform::DatetimeMs => "datetime",
            Transform::DatetimeUsec => "datetime_usec",
            Transform::Xml => "xml",
        }
    }
}

/// `RRDF_FIELD_SUMMARY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Summary {
    UniqueCount,
    Sum,
    Min,
    Max,
    Mean,
    Median,
    Count,
}

impl Summary {
    /// `RRDF_FIELD_SUMMARY_2str()`.
    pub fn name(self) -> &'static str {
        match self {
            Summary::UniqueCount => "uniqueCount",
            Summary::Sum => "sum",
            Summary::Min => "min",
            Summary::Max => "max",
            Summary::Mean => "mean",
            Summary::Median => "median",
            Summary::Count => "count",
        }
    }
}

/// `RRDF_FIELD_FILTER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    None,
    Range,
    MultiSelect,
    Facet,
}

impl Filter {
    /// `RRDF_FIELD_FILTER_2str()`.
    pub fn name(self) -> &'static str {
        match self {
            Filter::None => "none",
            Filter::Range => "range",
            Filter::MultiSelect => "multiselect",
            Filter::Facet => "facet",
        }
    }
}

/// `buffer_rrdf_table_add_field()`'s arguments. `None` strings are C's NULL.
#[derive(Debug, Clone, Copy)]
pub struct Field<'a> {
    pub id: usize,
    pub key: &'a [u8],
    pub name: &'a [u8],
    pub kind: FieldType,
    pub visual: Visual,
    pub transform: Transform,
    pub decimal_points: usize,
    pub units: Option<&'a [u8]>,
    /// NaN leaves `max` out.
    pub max: f64,
    pub sort: u8,
    pub pointer_to: Option<&'a [u8]>,
    pub summary: Summary,
    pub filter: Filter,
    pub options: u8,
    pub default_value: Option<&'a [u8]>,
}

/// `buffer_rrdf_table_add_field()`: the column as a member keyed by its key.
pub fn add_field(w: &mut JsonWriter, f: &Field<'_>) {
    let has = |bit: u8| f.options & bit != 0;
    w.member_add_object(f.key);
    w.member_add_uint64("index", f.id as u64);
    w.member_add_boolean("unique_key", has(opts::UNIQUE_KEY));
    w.member_add_string("name", f.name);
    w.member_add_boolean("visible", has(opts::VISIBLE));
    w.member_add_string("type", f.kind.name());
    w.member_add_string_or_omit("units", f.units);
    w.member_add_string("visualization", f.visual.name());
    w.member_add_object("value_options");
    w.member_add_string_or_omit("units", f.units);
    w.member_add_string("transform", f.transform.name());
    w.member_add_uint64("decimal_points", f.decimal_points as u64);
    w.member_add_string_opt("default_value", f.default_value);
    w.object_close();
    if !f.max.is_nan() {
        w.member_add_double("max", f.max);
    }
    w.member_add_string_or_omit("pointer_to", f.pointer_to);
    w.member_add_string("sort", sort::name(f.sort));
    w.member_add_boolean("sortable", f.sort & sort::FIXED == 0);
    w.member_add_boolean("sticky", has(opts::STICKY));
    w.member_add_string("summary", f.summary.name());
    w.member_add_string("filter", f.filter.name());
    w.member_add_boolean("full_width", has(opts::FULL_WIDTH));
    w.member_add_boolean("wrap", has(opts::WRAP));
    w.member_add_boolean("default_expanded_filter", has(opts::EXPANDED_FILTER));
    if has(opts::DUMMY) {
        w.member_add_boolean("dummy", true);
    }
    w.object_close();
}
