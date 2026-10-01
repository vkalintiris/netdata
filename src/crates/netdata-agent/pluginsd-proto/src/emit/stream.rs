//! The lines a streaming child sends its parent (`src/streaming/protocol/`, `stream-replication-sender.c`): the chart
//! definitions, the data in v2 and v1, the session's lines and the replication answers, byte for byte as C writes
//! them (map `knowledge/map-m7-commit2-codecs.md` in the status repository). Nothing is escaped, as in C. Each emitter
//! appends to `out`; the callers apply the capabilities they negotiated and hold the values.

use netdata_agent_text::c::double_to_i64;
use netdata_agent_text::print::{
    NumberEncoding, print_fixed, print_int64, print_int64_encoded, print_netdata_double_encoded,
    print_netdata_double_or_null, print_uint64, print_uint64_encoded, print_uuid_lower,
};

use crate::caps;

/// How slots and numbers print under a connection's capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enc {
    pub slots: bool,
    pub int: NumberEncoding,
    pub dbl: NumberEncoding,
}

impl Enc {
    /// Definitions and live data: base64 with IEEE754, else hex integers and decimal doubles.
    pub fn live(capabilities: u32) -> Enc {
        let ieee = capabilities & caps::IEEE754 != 0;
        Enc {
            slots: capabilities & caps::SLOTS != 0,
            int: if ieee { NumberEncoding::Base64 } else { NumberEncoding::Hex },
            dbl: if ieee { NumberEncoding::Base64 } else { NumberEncoding::Decimal },
        }
    }

    /// Replication answers: base64 with IEEE754, else decimal, never hex.
    pub fn replication(capabilities: u32) -> Enc {
        let ieee = capabilities & caps::IEEE754 != 0;
        let encoding = if ieee { NumberEncoding::Base64 } else { NumberEncoding::Decimal };
        Enc { slots: capabilities & caps::SLOTS != 0, int: encoding, dbl: encoding }
    }

    /// ` SLOT:<slot>` with SLOTS.
    fn slot(&self, out: &mut Vec<u8>, slot: u64) {
        if self.slots {
            out.extend_from_slice(b" SLOT:");
            print_uint64_encoded(out, self.int, slot);
        }
    }
}

fn s(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(text.as_bytes());
}

/// `"<text>"` after a space.
fn quoted(out: &mut Vec<u8>, text: &str) {
    quoted_bytes(out, text.as_bytes());
}

fn quoted_bytes(out: &mut Vec<u8>, text: &[u8]) {
    out.extend_from_slice(b" \"");
    out.extend_from_slice(text);
    out.push(b'"');
}

/// `%d`.
fn int(out: &mut Vec<u8>, value: i32) {
    print_int64(out, i64::from(value));
}

/// A chart as `CHART` announces it.
#[derive(Debug, Clone, Copy)]
pub struct ChartDef<'a> {
    pub slot: u64,
    /// `type.id`.
    pub id: &'a str,
    /// The name after its type (`type.name` without `type.`), empty when the name is the id.
    pub name: &'a str,
    pub title: &'a str,
    pub units: &'a str,
    pub family: &'a str,
    pub context: &'a str,
    pub chart_type: &'a str,
    pub priority: i32,
    pub update_every: i32,
    pub obsolete: bool,
    pub store_first: bool,
    pub hidden: bool,
    pub plugin: &'a str,
    pub module: &'a str,
}

/// `stream_sender_send_rrdset_definition()`'s name: empty when the chart has none or it is the id, else what follows
/// the first `.` of `type.name` (empty without one).
pub fn chart_name<'a>(id: &str, full_name: Option<&'a str>) -> &'a str {
    match full_name {
        Some(name) if name != id => name.split_once('.').map_or("", |(_, rest)| rest),
        _ => "",
    }
}

/// `CHART[ SLOT:<s>] "<id>" "<name>" "<title>" "<units>" "<family>" "<context>" "<type>" <priority> <update_every>
/// "<obsolete> <store_first> <hidden>" "<plugin>" "<module>"`.
pub fn chart(out: &mut Vec<u8>, e: &Enc, c: &ChartDef<'_>) {
    s(out, "CHART");
    e.slot(out, c.slot);
    for text in [c.id, c.name, c.title, c.units, c.family, c.context, c.chart_type] {
        quoted(out, text);
    }
    out.push(b' ');
    int(out, c.priority);
    out.push(b' ');
    int(out, c.update_every);
    out.extend_from_slice(b" \"");
    s(out, if c.obsolete { "obsolete" } else { "" });
    out.push(b' ');
    s(out, if c.store_first { "store_first" } else { "" });
    out.push(b' ');
    s(out, if c.hidden { "hidden" } else { "" });
    out.push(b'"');
    quoted(out, c.plugin);
    quoted(out, c.module);
    out.push(b'\n');
}

/// `CLABEL "<name>" "<value>" <source without the internal bits>`.
pub fn clabel(out: &mut Vec<u8>, name: &[u8], value: &[u8], source: i32) {
    s(out, "CLABEL");
    quoted_bytes(out, name);
    quoted_bytes(out, value);
    out.push(b' ');
    int(out, source);
    out.push(b'\n');
}

/// `CLABEL_COMMIT`, after at least one CLABEL.
pub fn clabel_commit(out: &mut Vec<u8>) {
    s(out, "CLABEL_COMMIT\n");
}

/// A dimension as `DIMENSION` announces it.
#[derive(Debug, Clone, Copy)]
pub struct DimDef<'a> {
    pub slot: u64,
    pub id: &'a str,
    pub name: &'a str,
    pub algorithm: &'a str,
    pub multiplier: i32,
    pub divisor: i32,
    pub obsolete: bool,
    pub hidden: bool,
    pub noreset: bool,
    pub float: bool,
}

/// `DIMENSION[ SLOT:<s>] "<id>" "<name>" "<algorithm>" <multiplier> <divisor> "<obsolete> <hidden> <noreset>
/// <type=float|type=int>"`.
pub fn dimension(out: &mut Vec<u8>, e: &Enc, d: &DimDef<'_>) {
    s(out, "DIMENSION");
    e.slot(out, d.slot);
    for text in [d.id, d.name, d.algorithm] {
        quoted(out, text);
    }
    out.push(b' ');
    int(out, d.multiplier);
    out.push(b' ');
    int(out, d.divisor);
    out.extend_from_slice(b" \"");
    s(out, if d.obsolete { "obsolete" } else { "" });
    out.push(b' ');
    s(out, if d.hidden { "hidden" } else { "" });
    out.push(b' ');
    s(out, if d.noreset { "noreset" } else { "" });
    out.push(b' ');
    s(out, if d.float { "type=float" } else { "type=int" });
    out.extend_from_slice(b"\"\n");
}

/// A custom variable's scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarScope {
    Host,
    Chart,
}

/// `VARIABLE HOST|CHART <name> = <%0.7f>`.
pub fn variable(out: &mut Vec<u8>, scope: VarScope, name: &str, value: f64) {
    s(out, if scope == VarScope::Host { "VARIABLE HOST " } else { "VARIABLE CHART " });
    s(out, name);
    out.extend_from_slice(b" = ");
    print_fixed(out, value, 7);
    out.push(b'\n');
}

/// `CHART_DEFINITION_END <db first> <db last> <now>`, always decimal (C's `%llu` of the times).
pub fn chart_definition_end(out: &mut Vec<u8>, db_first_s: i64, db_last_s: i64, now_s: i64) {
    s(out, "CHART_DEFINITION_END ");
    print_uint64(out, db_first_s as u64);
    out.push(b' ');
    print_uint64(out, db_last_s as u64);
    out.push(b' ');
    print_uint64(out, now_s as u64);
    out.push(b'\n');
}

/// A dimension's last collected value as SET2 and RDSTATE carry it: a double for a float dimension with
/// FLOAT_BASELINE, else the integer (`rrddim_last_collected_raw_int()`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Baseline {
    Int(i64),
    Float(f64),
}

impl Baseline {
    fn print(self, out: &mut Vec<u8>, e: &Enc) {
        match self {
            Baseline::Int(v) => print_int64_encoded(out, e.int, v),
            Baseline::Float(v) => print_netdata_double_encoded(out, e.dbl, v),
        }
    }

    fn as_double(self) -> f64 {
        match self {
            Baseline::Int(v) => v as f64,
            Baseline::Float(v) => v,
        }
    }
}

/// `RRDSET_STREAM_BUFFER`'s v2 half: one chart's BEGIN2/SET2 lines of a collection, a block per stored point.
#[derive(Debug, Clone)]
pub struct V2Block<'a> {
    enc: Enc,
    chart_slot: u64,
    chart_id: &'a str,
    update_every: u64,
    wall_clock_s: i64,
    last_end_s: i64,
    open: bool,
}

impl<'a> V2Block<'a> {
    /// A chart's block of the collection at `wall_clock_s` (`rsb.wall_clock_time`).
    pub fn new(enc: Enc, chart_slot: u64, chart_id: &'a str, update_every: u64, wall_clock_s: i64) -> Self {
        V2Block { enc, chart_slot, chart_id, update_every, wall_clock_s, last_end_s: 0, open: false }
    }

    /// `stream_send_rrddim_metrics_v2()`: a SET2 of the point ending at `end_s`, after a BEGIN2 when the point's time
    /// changed (an END2 first when a block is open). Nothing for a value that is no number or an empty slot (`E`).
    #[allow(clippy::too_many_arguments)]
    pub fn set2(
        &mut self,
        out: &mut Vec<u8>,
        end_s: i64,
        dim_slot: u64,
        dim_id: &str,
        baseline: Baseline,
        value: f64,
        flags: &[u8],
    ) {
        if !value.is_finite() || flags == b"E" {
            return;
        }
        let e = self.enc;
        if self.last_end_s != end_s {
            if self.open {
                end2(out);
            }
            s(out, "BEGIN2");
            e.slot(out, self.chart_slot);
            out.extend_from_slice(b" '");
            s(out, self.chart_id);
            out.extend_from_slice(b"' ");
            print_uint64_encoded(out, e.int, self.update_every);
            out.push(b' ');
            print_uint64_encoded(out, e.int, end_s as u64);
            out.push(b' ');
            if end_s == self.wall_clock_s {
                out.push(b'#');
            } else {
                print_uint64_encoded(out, e.int, self.wall_clock_s as u64);
            }
            out.push(b'\n');
            self.last_end_s = end_s;
            self.open = true;
        }
        s(out, "SET2");
        e.slot(out, dim_slot);
        out.extend_from_slice(b" '");
        s(out, dim_id);
        out.extend_from_slice(b"' ");
        baseline.print(out, &e);
        out.push(b' ');
        if baseline.as_double() == value {
            out.push(b'#');
        } else {
            print_netdata_double_encoded(out, e.dbl, value);
        }
        out.push(b' ');
        out.extend_from_slice(flags);
        out.push(b'\n');
    }

    /// Whether a BEGIN2 went out: the finish then writes the chart's variables when flagged, and END2.
    pub fn is_open(&self) -> bool {
        self.open
    }
}

/// `END2`.
pub fn end2(out: &mut Vec<u8>) {
    s(out, "END2\n");
}

/// How a proxy forwards a child's BEGIN2 and SET2 lines (`pluginsd_begin_v2()`, `pluginsd_set_v2()`): in the
/// parent's slots and encodings, the child's words copied when both links read numbers alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Forward {
    /// The parent's slots and encodings.
    pub enc: Enc,
    /// Both links agree on IEEE754 (`can_copy`).
    pub same_numbers: bool,
    /// The parent takes a float dimension's baseline as a double (FLOAT_BASELINE).
    pub parent_float: bool,
}

impl Forward {
    /// Between a child that negotiated `child_caps` and a parent that negotiated `parent_caps`.
    pub fn new(child_caps: u32, parent_caps: u32) -> Forward {
        Forward {
            enc: Enc::live(parent_caps),
            same_numbers: (child_caps & caps::IEEE754 != 0) == (parent_caps & caps::IEEE754 != 0),
            parent_float: parent_caps & caps::FLOAT_BASELINE != 0,
        }
    }
}

/// A number as the child sent it: its word, and the value the parser read from it.
#[derive(Debug, Clone, Copy)]
pub struct Sent<'a, T> {
    pub word: &'a [u8],
    pub value: T,
}

/// A forwarded BEGIN2 (an END2 first when the previous block is still `open`): the update every copied only while
/// the chart kept the child's, the end and wall clock copied (`#` too) or encoded; the chart's slot is the proxy's.
#[allow(clippy::too_many_arguments)]
pub fn begin2_forward(
    out: &mut Vec<u8>,
    f: &Forward,
    open: bool,
    chart_slot: u64,
    chart_id: &str,
    update_every: Sent<'_, u64>,
    chart_update_every: u64,
    end: Sent<'_, u64>,
    wall: Sent<'_, u64>,
) {
    if open {
        end2(out);
    }
    s(out, "BEGIN2");
    f.enc.slot(out, chart_slot);
    out.extend_from_slice(b" '");
    s(out, chart_id);
    out.extend_from_slice(b"' ");
    if f.same_numbers && chart_update_every == update_every.value {
        out.extend_from_slice(update_every.word);
    } else {
        print_uint64_encoded(out, f.enc.int, chart_update_every);
    }
    for n in [end, wall] {
        out.push(b' ');
        if f.same_numbers {
            out.extend_from_slice(n.word);
        } else {
            print_uint64_encoded(out, f.enc.int, n.value);
        }
    }
    out.push(b'\n');
}

/// A forwarded SET2: the child's words copied unless the links differ in IEEE754 or in whether this dimension's
/// baseline is a double (`collected` is a [`Baseline::Float`] when the child sent one); else the baseline as a double
/// or C's cast to an integer and an explicit value. `flags` are rendered again, anomaly bit included.
#[allow(clippy::too_many_arguments)]
pub fn set2_forward(
    out: &mut Vec<u8>,
    f: &Forward,
    dim_slot: u64,
    dim_id: &str,
    is_float: bool,
    collected: Sent<'_, Baseline>,
    value: Sent<'_, f64>,
    flags: &[u8],
) {
    let parent_float = is_float && f.parent_float;
    let copy = f.same_numbers && matches!(collected.value, Baseline::Float(_)) == parent_float;
    s(out, "SET2");
    f.enc.slot(out, dim_slot);
    out.extend_from_slice(b" '");
    s(out, dim_id);
    out.extend_from_slice(b"' ");
    if copy {
        out.extend_from_slice(collected.word);
    } else if parent_float {
        print_netdata_double_encoded(out, f.enc.dbl, collected.value.as_double());
    } else {
        let v = match collected.value {
            Baseline::Int(v) => v,
            Baseline::Float(v) => double_to_i64(v),
        };
        print_int64_encoded(out, f.enc.int, v);
    }
    out.push(b' ');
    if copy {
        out.extend_from_slice(value.word);
    } else {
        print_netdata_double_encoded(out, f.enc.dbl, value.value);
    }
    out.push(b' ');
    out.extend_from_slice(flags);
    out.push(b'\n');
}

/// `BEGIN "<chart id>" <microseconds since the last update>` (v1, 0 until the chart resynced).
pub fn v1_begin(out: &mut Vec<u8>, chart_id: &str, usec_since_last_update: u64) {
    s(out, "BEGIN \"");
    s(out, chart_id);
    out.extend_from_slice(b"\" ");
    print_uint64(out, usec_since_last_update);
    out.push(b'\n');
}

/// A v1 SET's value: the collected integer, or a float dimension's double with FLOAT_BASELINE (`null` when no number).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum V1Value {
    Int(i64),
    Float(f64),
}

/// `SET "<dim id>" = <value>`.
pub fn v1_set(out: &mut Vec<u8>, dim_id: &str, value: V1Value) {
    s(out, "SET \"");
    s(out, dim_id);
    out.extend_from_slice(b"\" = ");
    match value {
        V1Value::Int(v) => print_int64(out, v),
        V1Value::Float(v) => print_netdata_double_or_null(out, v),
    }
    out.push(b'\n');
}

/// `END`.
pub fn v1_end(out: &mut Vec<u8>) {
    s(out, "END\n");
}

/// `LABEL "<name>" = <source> "<value>"` (a host label).
pub fn label(out: &mut Vec<u8>, name: &[u8], source: u32, value: &[u8]) {
    s(out, "LABEL \"");
    out.extend_from_slice(name);
    out.extend_from_slice(b"\" = ");
    print_uint64(out, u64::from(source));
    quoted_bytes(out, value);
    out.push(b'\n');
}

/// `OVERWRITE labels`, after the host labels (even none).
pub fn overwrite_labels(out: &mut Vec<u8>) {
    s(out, "OVERWRITE labels\n");
}

/// `CLAIMED_ID '<machine guid>' '<claim id|NULL>'`.
pub fn claimed_id(out: &mut Vec<u8>, machine_guid: &str, claim_id: Option<&[u8; 16]>) {
    s(out, "CLAIMED_ID '");
    s(out, machine_guid);
    out.extend_from_slice(b"' '");
    match claim_id {
        Some(id) => print_uuid_lower(out, id),
        None => s(out, "NULL"),
    }
    out.extend_from_slice(b"'\n");
}

/// `FUNCTION GLOBAL "<name>" <timeout> "<help>" "<tags>" 0x<access> <priority> <version>`.
#[allow(clippy::too_many_arguments)]
pub fn function_global(
    out: &mut Vec<u8>,
    name: &[u8],
    timeout_s: i32,
    help: &[u8],
    tags: &[u8],
    access: u32,
    priority: i32,
    version: u32,
) {
    use std::io::Write as _;
    s(out, "FUNCTION GLOBAL");
    quoted_bytes(out, name);
    out.push(b' ');
    int(out, timeout_s);
    quoted_bytes(out, help);
    quoted_bytes(out, tags);
    let _ = write!(out, " 0x{access:x} ");
    int(out, priority);
    out.push(b' ');
    print_uint64(out, u64::from(version));
    out.push(b'\n');
}

/// `FUNCTION_DEL GLOBAL "<name>"`.
pub fn function_del_global(out: &mut Vec<u8>, name: &str) {
    s(out, "FUNCTION_DEL GLOBAL");
    quoted(out, name);
    out.push(b'\n');
}

/// DynCfg's function, as `dyncfg_add_streaming()` announces it.
pub fn function_config_global(out: &mut Vec<u8>) {
    s(out, "FUNCTION GLOBAL config 120 \"Dynamic configuration\" \"config\" 0x8 1000\n");
}

/// `NODE_ID '<claim id>' '<node id>' '<cloud url>'` (a parent's, to its child).
pub fn node_id(out: &mut Vec<u8>, claim_id: &[u8; 16], node_id: &[u8; 16], cloud_url: &str) {
    s(out, "NODE_ID '");
    print_uuid_lower(out, claim_id);
    out.extend_from_slice(b"' '");
    print_uuid_lower(out, node_id);
    out.extend_from_slice(b"' '");
    s(out, cloud_url);
    out.extend_from_slice(b"'\n");
}

/// `RBEGIN[ SLOT:<s>] '<chart id>'`: a chart's replication answer.
pub fn rbegin_chart(out: &mut Vec<u8>, e: &Enc, chart_slot: u64, chart_id: &str) {
    s(out, "RBEGIN");
    e.slot(out, chart_slot);
    out.extend_from_slice(b" '");
    s(out, chart_id);
    out.extend_from_slice(b"'\n");
}

/// `RBEGIN[ SLOT:<s>] '' <start> <end> <wall>`: one replicated step.
pub fn rbegin_point(out: &mut Vec<u8>, e: &Enc, chart_slot: u64, start_s: i64, end_s: i64, wall_s: i64) {
    s(out, "RBEGIN");
    e.slot(out, chart_slot);
    out.extend_from_slice(b" '' ");
    print_uint64_encoded(out, e.int, start_s as u64);
    out.push(b' ');
    print_uint64_encoded(out, e.int, end_s as u64);
    out.push(b' ');
    print_uint64_encoded(out, e.int, wall_s as u64);
    out.push(b'\n');
}

/// `RSET[ SLOT:<s>] "<dim id>" <sum> <flags>`.
pub fn rset(out: &mut Vec<u8>, e: &Enc, dim_slot: u64, dim_id: &str, sum: f64, flags: &[u8]) {
    s(out, "RSET");
    e.slot(out, dim_slot);
    quoted(out, dim_id);
    out.push(b' ');
    print_netdata_double_encoded(out, e.dbl, sum);
    out.push(b' ');
    out.extend_from_slice(flags);
    out.push(b'\n');
}

/// `RDSTATE[ SLOT:<s>] '<dim id>' <last collected ut> <baseline> <last calculated> <last stored>`.
#[allow(clippy::too_many_arguments)]
pub fn rdstate(
    out: &mut Vec<u8>,
    e: &Enc,
    dim_slot: u64,
    dim_id: &str,
    last_collected_ut: u64,
    baseline: Baseline,
    last_calculated: f64,
    last_stored: f64,
) {
    s(out, "RDSTATE");
    e.slot(out, dim_slot);
    out.extend_from_slice(b" '");
    s(out, dim_id);
    out.extend_from_slice(b"' ");
    print_uint64_encoded(out, e.int, last_collected_ut);
    out.push(b' ');
    baseline.print(out, e);
    out.push(b' ');
    print_netdata_double_encoded(out, e.dbl, last_calculated);
    out.push(b' ');
    print_netdata_double_encoded(out, e.dbl, last_stored);
    out.push(b'\n');
}

/// `RSSTATE <last collected ut> <last updated ut>` (the chart's).
pub fn rsstate(out: &mut Vec<u8>, e: &Enc, last_collected_ut: u64, last_updated_ut: u64) {
    s(out, "RSSTATE ");
    print_uint64_encoded(out, e.int, last_collected_ut);
    out.push(b' ');
    print_uint64_encoded(out, e.int, last_updated_ut);
    out.push(b'\n');
}

/// `REND <update every> <db first> <db last> true|false <after> <before> <wall>`, `true` followed by two spaces as C
/// writes it.
#[allow(clippy::too_many_arguments)]
pub fn rend(
    out: &mut Vec<u8>,
    e: &Enc,
    update_every: i64,
    db_first_s: i64,
    db_last_s: i64,
    start_streaming: bool,
    after_s: i64,
    before_s: i64,
    wall_s: i64,
) {
    s(out, "REND ");
    print_int64_encoded(out, e.int, update_every);
    out.push(b' ');
    print_uint64_encoded(out, e.int, db_first_s as u64);
    out.push(b' ');
    print_uint64_encoded(out, e.int, db_last_s as u64);
    s(out, if start_streaming { " true  " } else { " false " });
    print_uint64_encoded(out, e.int, after_s as u64);
    out.push(b' ');
    print_uint64_encoded(out, e.int, before_s as u64);
    out.push(b' ');
    print_uint64_encoded(out, e.int, wall_s as u64);
    out.push(b'\n');
}

#[cfg(test)]
mod tests {
    use netdata_agent_text::parse::{str2ll_encoded, str2ndd_encoded, str2ull_encoded};

    use super::*;

    const LIVE: u32 = caps::SLOTS | caps::IEEE754;

    fn text(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut out = Vec::new();
        f(&mut out);
        String::from_utf8(out).unwrap()
    }

    fn u(s: &str) -> u64 {
        str2ull_encoded(s.as_bytes())
    }

    fn d(s: &str) -> f64 {
        str2ndd_encoded(s.as_bytes()).0
    }

    /// C writes ids, names, titles and label values between quotes as they are (`"%s"`): a quote or a newline in a
    /// value goes out unescaped and corrupts the line, as C's.
    #[test]
    fn values_go_unescaped_as_c() {
        let e = Enc::live(0);
        let def = ChartDef {
            slot: 0,
            id: "a.b",
            name: "",
            title: "say \"hi\"",
            units: "u",
            family: "f",
            context: "a.ctx",
            chart_type: "line",
            priority: 1,
            update_every: 1,
            obsolete: false,
            store_first: false,
            hidden: false,
            plugin: "p",
            module: "m",
        };
        assert_eq!(
            text(|o| chart(o, &e, &def)),
            "CHART \"a.b\" \"\" \"say \"hi\"\" \"u\" \"f\" \"a.ctx\" \"line\" 1 1 \"  \" \"p\" \"m\"\n"
        );
        let dim = DimDef {
            slot: 0,
            id: "d",
            name: "d\"x",
            algorithm: "absolute",
            multiplier: 1,
            divisor: 1,
            obsolete: false,
            hidden: false,
            noreset: false,
            float: false,
        };
        assert_eq!(text(|o| dimension(o, &e, &dim)), "DIMENSION \"d\" \"d\"x\" \"absolute\" 1 1 \"   type=int\"\n");
        assert_eq!(text(|o| clabel(o, b"k", b"v\"1", 1)), "CLABEL \"k\" \"v\"1\" 1\n");
        assert_eq!(text(|o| clabel(o, b"k", b"v\n2", 1)), "CLABEL \"k\" \"v\n2\" 1\n");
        assert_eq!(text(|o| label(o, b"k", 1, b"v\"1")), "LABEL \"k\" = 1 \"v\"1\"\n");
    }

    /// Lines of a C child's capture (SLOTS and IEEE754), rebuilt from their values.
    #[test]
    fn definitions_as_a_c_child() {
        let e = Enc::live(LIVE);
        let def = ChartDef {
            slot: 2,
            id: "netdata.clients",
            name: chart_name("netdata.clients", Some("netdata.clients")),
            title: "Netdata Web API Clients",
            units: "connected clients",
            family: "HTTP API",
            context: "netdata.http_api_clients",
            chart_type: "line",
            priority: 130200,
            update_every: 1,
            obsolete: false,
            store_first: false,
            hidden: false,
            plugin: "netdata",
            module: "pulse",
        };
        assert_eq!(
            text(|o| chart(o, &e, &def)),
            "CHART SLOT:#C \"netdata.clients\" \"\" \"Netdata Web API Clients\" \"connected clients\" \"HTTP API\" \
             \"netdata.http_api_clients\" \"line\" 130200 1 \"  \" \"netdata\" \"pulse\"\n"
        );
        let dim = DimDef {
            slot: 1,
            id: "tier0",
            name: "tier0",
            algorithm: "incremental",
            multiplier: 1,
            divisor: 1,
            obsolete: false,
            hidden: false,
            noreset: false,
            float: false,
        };
        assert_eq!(text(|o| dimension(o, &e, &dim)), "DIMENSION SLOT:#B \"tier0\" \"tier0\" \"incremental\" 1 1 \"   type=int\"\n");
        assert_eq!(text(|o| clabel(o, b"_collect_plugin", b"netdata", 1)), "CLABEL \"_collect_plugin\" \"netdata\" 1\n");
        assert_eq!(
            text(|o| chart_definition_end(o, 1790619274, 1790619280, 1790619281)),
            "CHART_DEFINITION_END 1790619274 1790619280 1790619281\n"
        );
        assert_eq!(
            text(|o| claimed_id(o, "5a1e0000-0000-4000-8000-0000000000cc", None)),
            "CLAIMED_ID '5a1e0000-0000-4000-8000-0000000000cc' 'NULL'\n"
        );
        // hex without IEEE754, flags words, a name after its type
        let mut hidden = def;
        hidden.slot = 26;
        hidden.name = chart_name("a.b", Some("a.b.c"));
        hidden.obsolete = true;
        hidden.hidden = true;
        let got = text(|o| chart(o, &Enc::live(caps::SLOTS), &hidden));
        assert!(got.starts_with("CHART SLOT:0x1A \"netdata.clients\" \"b.c\" "), "{got}");
        assert!(got.ends_with(" 130200 1 \"obsolete  hidden\" \"netdata\" \"pulse\"\n"), "{got}");
        let float = DimDef { float: true, noreset: true, ..dim };
        assert!(text(|o| dimension(o, &Enc::live(0), &float)).ends_with(" 1 1 \"  noreset type=float\"\n"));
        assert_eq!(chart_name("a.b", None), "");
        assert_eq!(chart_name("a.b", Some("nodot")), "");
    }

    /// A v2 block as C writes it: `#` for the wall clock and a value equal to its baseline, END2 between points.
    #[test]
    fn v2_as_a_c_child() {
        let e = Enc::live(LIVE);
        let end = u("#Bqurzm") as i64;
        let mut b = V2Block::new(e, 1, "netdata.db_samples_collected", 1, end);
        let got = text(|o| {
            b.set2(o, end, 1, "tier0", Baseline::Int(str2ll_encoded(b"#II")), d("@EBQP/mcna4P"), b"A");
            b.set2(o, end, 2, "clients", Baseline::Int(0), 0.0, b"A");
            b.set2(o, end, 3, "nan", Baseline::Int(0), f64::NAN, b"A");
            b.set2(o, end, 3, "empty", Baseline::Int(0), 1.0, b"E");
            b.set2(o, end - 1, 1, "tier0", Baseline::Int(-2), 1.5, b"''");
            end2(o);
        });
        assert_eq!(
            got,
            "BEGIN2 SLOT:#B 'netdata.db_samples_collected' #B #Bqurzm #\nSET2 SLOT:#B 'tier0' #II @EBQP/mcna4P A\n\
             SET2 SLOT:#C 'clients' #A # A\nEND2\nBEGIN2 SLOT:#B 'netdata.db_samples_collected' #B #Bqurzl #Bqurzm\n\
             SET2 SLOT:#B 'tier0' -#C @D/4AAAAAAAA ''\nEND2\n"
        );
        assert!(b.is_open());
        // no IEEE754: hex integers, decimal doubles, a float baseline
        let mut b = V2Block::new(Enc::live(0), 0, "c", 2, 100);
        let got = text(|o| b.set2(o, 90, 0, "d", Baseline::Float(1.25), 0.5, b"AR"));
        assert_eq!(got, "BEGIN2 'c' 0x2 0x5A 0x64\nSET2 'd' 1.25 0.5 AR\n");
    }

    /// A block as a C proxy forwards it: each variant's lines of `TestProxyTranscript`'s upstream capture
    /// (`evidence/2026-09-29-proxy-golden/` in the status repository), rebuilt from the child's words.
    #[test]
    fn forward_as_a_c_proxy() {
        const PROXY: u32 = caps::SLOTS | caps::IEEE754 | caps::FLOAT_BASELINE;
        const PLAIN: u32 = caps::FLOAT_BASELINE;
        const C_LIKE: u32 = caps::FLOAT_BASELINE | caps::IEEE754 | caps::SLOTS;
        let b64 = |v: u64| text(|o| print_uint64_encoded(o, NumberEncoding::Base64, v));
        // the parser's reading of BEGIN2's words: `#` is the end time
        let begin2 = |f: &Forward, open: bool, id: &str, ue: &str, chart_ue: u64, end: &str, wall: &str| {
            let end_s = Sent { word: end.as_bytes(), value: u(end) };
            let wall_v = if wall == "#" { u(end) } else { u(wall) };
            text(|o| {
                begin2_forward(
                    o,
                    f,
                    open,
                    1,
                    id,
                    Sent { word: ue.as_bytes(), value: u(ue) },
                    chart_ue,
                    end_s,
                    Sent { word: wall.as_bytes(), value: wall_v },
                )
            })
        };
        // SET2's: a double baseline when the child sent one for a float dimension, `#` the baseline, an empty slot
        // or no number a NaN
        let set2 = |f: &Forward, slot: u64, id: &str, float: bool, sent_float: bool, c: &str, v: &str, flags: &str| {
            let collected =
                if sent_float { Baseline::Float(d(c)) } else { Baseline::Int(str2ll_encoded(c.as_bytes())) };
            let value = if v == "#" { collected.as_double() } else { d(v) };
            let value = if flags == "E" || !value.is_finite() { f64::NAN } else { value };
            text(|o| {
                set2_forward(
                    o,
                    f,
                    slot,
                    id,
                    float,
                    Sent { word: c.as_bytes(), value: collected },
                    Sent { word: v.as_bytes(), value },
                    flags.as_bytes(),
                )
            })
        };

        // copy: neither link has IEEE754, the words go as sent (the proxy renders `RA` as `AR`)
        let f = Forward::new(PLAIN, PROXY & !caps::IEEE754);
        assert_eq!(
            begin2(&f, false, "proxy.gauge", "1", 1, "1790690343", "#"),
            "BEGIN2 SLOT:0x1 'proxy.gauge' 1 1790690343 #\n"
        );
        assert_eq!(set2(&f, 1, "g1", false, false, "1", "1", "A"), "SET2 SLOT:0x1 'g1' 1 1 A\n");
        assert_eq!(set2(&f, 2, "g2", false, false, "2", "#", "AR"), "SET2 SLOT:0x2 'g2' 2 # AR\n");
        assert_eq!(set2(&f, 1, "i1", false, false, "1010", "10", "''"), "SET2 SLOT:0x1 'i1' 1010 10 ''\n");
        assert_eq!(set2(&f, 1, "f1", true, true, "1", "1.25", "A"), "SET2 SLOT:0x1 'f1' 1 1.25 A\n");
        assert_eq!(set2(&f, 1, "f1", true, true, "0", "NAN", "E"), "SET2 SLOT:0x1 'f1' 0 NAN E\n");

        // reencode-up: base64 for a parent with IEEE754, the wall clock explicit
        let f = Forward::new(PLAIN, PROXY);
        let end = u("#Bqu8Sf").to_string();
        assert_eq!(
            begin2(&f, false, "proxy.gauge", "1", 1, &end, "#"),
            "BEGIN2 SLOT:#B 'proxy.gauge' #B #Bqu8Sf #Bqu8Sf\n"
        );
        assert_eq!(set2(&f, 1, "g1", false, false, "1", "1", "A"), "SET2 SLOT:#B 'g1' #B @D/wAAAAAAAA A\n");
        assert_eq!(set2(&f, 2, "g2", false, false, "2", "#", "AR"), "SET2 SLOT:#C 'g2' #C @EAAAAAAAAAA AR\n");
        assert_eq!(set2(&f, 1, "i1", false, false, "1010", "10", "''"), "SET2 SLOT:#B 'i1' #Py @EAkAAAAAAAA ''\n");
        assert_eq!(set2(&f, 1, "f1", true, true, "1", "1.25", "A"), "SET2 SLOT:#B 'f1' @D/wAAAAAAAA @D/0AAAAAAAA A\n");
        assert_eq!(set2(&f, 1, "f1", true, true, "0", "NAN", "E"), "SET2 SLOT:#B 'f1' @A @H/4AAAAAAAA E\n");

        // reencode-down: a base64 child, a parent without IEEE754: hex and decimal, NaN as `null`
        let f = Forward::new(C_LIKE, PROXY & !caps::IEEE754);
        let end = b64(0x6ABB_C463);
        assert_eq!(
            begin2(&f, false, "proxy.gauge", "#B", 1, &end, "#"),
            "BEGIN2 SLOT:0x1 'proxy.gauge' 0x1 0x6ABBC463 0x6ABBC463\n"
        );
        assert_eq!(set2(&f, 1, "g1", false, false, "#B", "@D/wAAAAAAAA", "A"), "SET2 SLOT:0x1 'g1' 0x1 1 A\n");
        assert_eq!(set2(&f, 2, "g2", false, false, "#C", "#", "AR"), "SET2 SLOT:0x2 'g2' 0x2 2 AR\n");
        assert_eq!(set2(&f, 1, "i1", false, false, "#Py", "@EAkAAAAAAAA", "''"), "SET2 SLOT:0x1 'i1' 0x3F2 10 ''\n");
        assert_eq!(set2(&f, 1, "f1", true, true, "#B", "@D/0AAAAAAAA", "A"), "SET2 SLOT:0x1 'f1' 1 1.25 A\n");
        assert_eq!(set2(&f, 1, "f1", true, true, "#A", "NAN", "E"), "SET2 SLOT:0x1 'f1' 0 null E\n");

        // float-up: the child sent an integer baseline, the parent takes a double
        let f = Forward::new(0, PROXY);
        assert_eq!(set2(&f, 1, "f1", true, false, "1", "1.25", "A"), "SET2 SLOT:#B 'f1' @D/wAAAAAAAA @D/0AAAAAAAA A\n");
        assert_eq!(set2(&f, 1, "f1", true, false, "0", "NAN", "E"), "SET2 SLOT:#B 'f1' @A @H/4AAAAAAAA E\n");

        // float-down: the numbers agree but the parent takes an integer baseline: C's cast, in hex
        let f = Forward::new(PLAIN, PROXY & !(caps::IEEE754 | caps::FLOAT_BASELINE));
        assert_eq!(
            begin2(&f, false, "proxy.gauge", "1", 1, "1790690403", "#"),
            "BEGIN2 SLOT:0x1 'proxy.gauge' 1 1790690403 #\n"
        );
        assert_eq!(set2(&f, 1, "g1", false, false, "1", "1", "A"), "SET2 SLOT:0x1 'g1' 1 1 A\n");
        assert_eq!(set2(&f, 1, "f1", true, true, "1", "1.25", "A"), "SET2 SLOT:0x1 'f1' 0x1 1.25 A\n");
        assert_eq!(set2(&f, 1, "f1", true, true, "0", "NAN", "E"), "SET2 SLOT:0x1 'f1' 0x0 null E\n");
        assert_eq!(set2(&f, 1, "f1", true, true, "1.75", "#", "A"), "SET2 SLOT:0x1 'f1' 0x1 1.75 A\n");
        assert_eq!(set2(&f, 1, "f1", true, true, "-1.75", "#", "A"), "SET2 SLOT:0x1 'f1' -0x1 -1.75 A\n");

        // an open block closes first; an update every the chart did not take is the chart's; no slots without SLOTS
        let f = Forward::new(PLAIN, caps::FLOAT_BASELINE);
        assert_eq!(begin2(&f, true, "c", "1", 2, "10", "#"), "END2\nBEGIN2 'c' 0x2 10 #\n");
        let f = Forward::new(PLAIN, PROXY);
        assert_eq!(begin2(&f, false, "c", "2", 2, "10", "11"), "BEGIN2 SLOT:#B 'c' #C #K #L\n");
    }

    /// The replication lines of a C child's capture.
    #[test]
    fn replication_as_a_c_child() {
        let e = Enc::replication(LIVE);
        assert_eq!(
            text(|o| rdstate(o, &e, 1, "clients", u("#GXI/kzUQn"), Baseline::Int(0), 0.0, 0.0)),
            "RDSTATE SLOT:#B 'clients' #GXI/kzUQn #A @A @A\n"
        );
        assert_eq!(
            text(|o| rdstate(o, &e, 1, "tier0", u("#GXI/kzUOS"), Baseline::Int(520), d("@D92GSE3MYAA"), d("@EBQQAu2jOEt"))),
            "RDSTATE SLOT:#B 'tier0' #GXI/kzUOS #II @D92GSE3MYAA @EBQQAu2jOEt\n"
        );
        assert_eq!(text(|o| rsstate(o, &e, u("#GXI/kzUOT"), u("#GXI/kzUNA"))), "RSSTATE #GXI/kzUOT #GXI/kzUNA\n");
        let (first, last) = (u("#Bqurzc") as i64, u("#Bqurzl") as i64);
        assert_eq!(
            text(|o| rend(o, &e, 1, first, last, true, 0, 0, last)),
            "REND #B #Bqurzc #Bqurzl true  #A #A #Bqurzl\n"
        );
        assert_eq!(
            text(|o| rbegin_chart(o, &e, 1, "netdata.db_samples_collected")),
            "RBEGIN SLOT:#B 'netdata.db_samples_collected'\n"
        );
        // decimal without IEEE754, never hex
        let e = Enc::replication(caps::SLOTS);
        assert_eq!(text(|o| rbegin_point(o, &e, 26, 10, 11, 12)), "RBEGIN SLOT:26 '' 10 11 12\n");
        assert_eq!(text(|o| rset(o, &e, 3, "d", f64::NAN, b"A")), "RSET SLOT:3 \"d\" null A\n");
        assert_eq!(text(|o| rend(o, &e, 1, 0, 0, false, 5, 6, 7)), "REND 1 0 0 false 5 6 7\n");
    }

    #[test]
    fn session_and_v1_lines_as_c() {
        assert_eq!(text(|o| label(o, b"_os", 2147483649, b"linux")), "LABEL \"_os\" = 2147483649 \"linux\"\n");
        assert_eq!(text(overwrite_labels), "OVERWRITE labels\n");
        assert_eq!(text(|o| variable(o, VarScope::Host, "v", 0.00390625)), "VARIABLE HOST v = 0.0039062\n");
        assert_eq!(text(|o| variable(o, VarScope::Chart, "w", f64::NAN)), "VARIABLE CHART w = nan\n");
        assert_eq!(
            text(|o| function_global(o, b"f", 10, b"help", b"top", 0x3a, 100, 3)),
            "FUNCTION GLOBAL \"f\" 10 \"help\" \"top\" 0x3a 100 3\n"
        );
        assert_eq!(text(|o| function_del_global(o, "f")), "FUNCTION_DEL GLOBAL \"f\"\n");
        assert_eq!(
            text(function_config_global),
            "FUNCTION GLOBAL config 120 \"Dynamic configuration\" \"config\" 0x8 1000\n"
        );
        assert_eq!(
            text(|o| {
                v1_begin(o, "c", 1_000_000);
                v1_set(o, "a", V1Value::Int(-5));
                v1_set(o, "b", V1Value::Float(f64::INFINITY));
                v1_set(o, "f", V1Value::Float(0.5));
                v1_end(o);
            }),
            "BEGIN \"c\" 1000000\nSET \"a\" = -5\nSET \"b\" = null\nSET \"f\" = 0.5\nEND\n"
        );
        assert_eq!(
            text(|o| node_id(o, &[0x11; 16], &[0x22; 16], "https://app.netdata.cloud")),
            "NODE_ID '11111111-1111-1111-1111-111111111111' '22222222-2222-2222-2222-222222222222' \
             'https://app.netdata.cloud'\n"
        );
    }
}

