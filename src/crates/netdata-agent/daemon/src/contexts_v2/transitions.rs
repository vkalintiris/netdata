//! The alert log's transitions for `/api/v2/alert_transitions` and `/api/v3/alert_transitions`
//! (`api_v2_contexts_alert_transitions.c`): the nine facets with their counts over every row the query hands out,
//! the rows a request keeps (the newest `last` of them after its anchor), and their writer.

use std::borrow::Cow;
use std::collections::VecDeque;

use indexmap::IndexMap;
use netdata_agent_health::entry::entry_flags_to_json_array;
use netdata_agent_health::sql::{ConfigOptions, alert_config_members, status_name};
use netdata_agent_metadata::health_log::{AlertConfigRow, TransitionRow};
use netdata_agent_query::keys::Keys;
use netdata_agent_query::tables::contexts_options::{DEBUG, JSON_LONG_KEYS, MCP, RFC3339};
use netdata_agent_text::c::c_str;
use netdata_agent_text::json::JsonWriter;
use netdata_agent_text::print::uuid_lower_text;
use netdata_agent_text::simple_pattern::SimplePattern;

/// `alert_transition_facets[]` in the order of its enum (`ATF_*`), which is the order of the answer and of the
/// request's echo: the id, which is the request's parameter too, the name, and the `order` a client sorts by.
pub(super) const FACETS: [(&str, &str, u64); 9] = [
    ("f_status", "Alert Status", 1),
    ("f_class", "Alert Class", 4),
    ("f_type", "Alert Type", 2),
    ("f_component", "Alert Component", 5),
    ("f_role", "Recipient Role", 3),
    ("f_node", "Alert Node", 6),
    ("f_alert", "Alert Name", 7),
    ("f_instance", "Instance Name", 8),
    ("f_context", "Context", 9),
];

/// `ATF_NODE`: the facet whose options are hosts.
const NODE: usize = 5;

/// `strncpyz()` into a field of `size` bytes: the text up to its terminator, at most `size - 1` bytes of it.
fn cut(text: Option<&[u8]>, size: usize) -> Vec<u8> {
    let text = c_str(text.unwrap_or_default());
    text[..text.len().min(size - 1)].to_vec()
}

/// A row's text as C reads it: up to its terminator, empty for a NULL.
fn text<'t>(text: &'t Option<Cow<'_, [u8]>>) -> &'t [u8] {
    text.as_deref().map(c_str).unwrap_or_default()
}

/// A text as C's `*t ? t : NULL`.
fn non_empty(text: &[u8]) -> Option<&[u8]> {
    (!text.is_empty()).then_some(text)
}

/// `SQL_TRANSITION_DATA_*_STRING` and `RRD_ID_LENGTH_MAX`: the sizes of a kept row's text fields.
const SMALL: usize = 6 * 8;
const MEDIUM: usize = 12 * 8;
const BIG: usize = 512;
const ID: usize = 1200;

/// `struct sql_alert_transition_fixed_size`: a kept row, a copy with C's byte cuts.
#[derive(Debug, Clone, PartialEq)]
struct Kept {
    global_id: u64,
    transition_id: [u8; 16],
    config_hash_id: [u8; 16],
    alert_name: Vec<u8>,
    chart: Vec<u8>,
    chart_name: Vec<u8>,
    chart_context: Vec<u8>,
    recipient: Vec<u8>,
    units: Vec<u8>,
    exec: Vec<u8>,
    info: Vec<u8>,
    summary: Vec<u8>,
    classification: Vec<u8>,
    r#type: Vec<u8>,
    component: Vec<u8>,
    when_key: i64,
    duration: i64,
    non_clear_duration: i64,
    flags: u64,
    delay_up_to_timestamp: i64,
    exec_run_timestamp: i64,
    exec_code: i32,
    new_status: i32,
    old_status: i32,
    delay: i32,
    new_value: f64,
    old_value: f64,
    machine_guid: String,
}

impl Kept {
    /// `contexts_v2_alert_transition_dup()`.
    fn of(row: &TransitionRow<'_>, machine_guid: &str) -> Kept {
        let chart = cut(row.chart.as_deref(), ID);
        // a NULL name is the chart's id, as cut; an empty name stays empty
        let chart_name = row.chart_name.as_deref().map_or_else(|| chart.clone(), |name| cut(Some(name), ID));
        Kept {
            global_id: row.global_id as u64,
            transition_id: row.transition_id,
            config_hash_id: row.config_hash_id,
            alert_name: cut(row.alert_name.as_deref(), SMALL),
            chart,
            chart_name,
            chart_context: cut(row.chart_context.as_deref(), MEDIUM),
            recipient: cut(row.recipient.as_deref(), MEDIUM),
            units: cut(row.units.as_deref(), SMALL),
            exec: cut(row.exec.as_deref(), BIG),
            info: cut(row.info.as_deref(), BIG),
            summary: cut(row.summary.as_deref(), BIG),
            classification: cut(row.classification.as_deref(), SMALL),
            r#type: cut(row.r#type.as_deref(), SMALL),
            component: cut(row.component.as_deref(), SMALL),
            when_key: row.when_key,
            duration: row.duration,
            non_clear_duration: row.non_clear_duration,
            flags: row.flags as u64,
            delay_up_to_timestamp: row.delay_up_to_timestamp,
            exec_run_timestamp: row.exec_run_timestamp,
            exec_code: row.exec_code,
            new_status: row.new_status,
            old_status: row.old_status,
            delay: row.delay,
            new_value: row.new_value,
            old_value: row.old_value,
            machine_guid: machine_guid.to_owned(),
        }
    }
}

/// `alert_transitions_callback_data.operations`: what the keep did, printed with `debug`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Operations {
    first: u64,
    prepend: u64,
    append: u64,
    backwards: u64,
    forwards: u64,
    shifts: u64,
    skips_before: u64,
    skips_after: u64,
}

/// What a host lookup tells the writer: the host's name and its node id (nil when it has none).
pub(super) type HostFacts = (String, [u8; 16]);

/// `struct alert_transitions_callback_data`: the facets, the kept rows and the counters of one request.
pub(super) struct Collector<'a> {
    /// Each facet's pattern, when the request named values for it.
    patterns: [Option<SimplePattern>; 9],
    /// Each facet's values in the order the rows showed them, with how many rows count for each.
    options: [IndexMap<Vec<u8>, u32>; 9],
    /// localhost's default recipient: the role of a row that names none.
    default_recipient: &'a [u8],
    anchor: u64,
    max: u64,
    /// Newest first.
    kept: VecDeque<Kept>,
    /// C's `last_added`: where the next row's place is looked for from.
    last_added: usize,
    evaluated: u64,
    matched: u64,
    returned: u64,
    operations: Operations,
}

impl<'a> Collector<'a> {
    /// `facets`: the request's text for each facet, in [`FACETS`]' order. A pattern is the web's, exact and
    /// whatever the case; a text with no word in it is no pattern. `last` is how many rows are kept, `anchor` the
    /// global id the kept rows are newer than.
    pub(super) fn new(facets: &[Option<Vec<u8>>; 9], default_recipient: &'a [u8], last: u32, anchor: u64) -> Self {
        Collector {
            patterns: std::array::from_fn(|i| facets[i].as_deref().and_then(SimplePattern::from_web_nocase)),
            options: std::array::from_fn(|_| IndexMap::new()),
            default_recipient: c_str(default_recipient),
            anchor,
            max: u64::from(last),
            kept: VecDeque::new(),
            last_added: 0,
            evaluated: 0,
            matched: 0,
            returned: 0,
            operations: Operations::default(),
        }
    }

    /// `contexts_v2_alert_transition_callback()`: one row of the query. Its nine values become options of their
    /// facets whatever happens next. A row every facet selects is kept, if it fits, and counts on every facet; a
    /// row all facets but one select counts on that one alone, which is what makes a facet show what choosing
    /// another of its values would give.
    pub(super) fn row(&mut self, row: &TransitionRow<'_>) {
        self.evaluated += 1;
        let (guid, len) = uuid_lower_text(&row.host_id, false);
        let role = Some(text(&row.recipient)).filter(|role| !role.is_empty()).unwrap_or(self.default_recipient);
        let values: [&[u8]; 9] = [
            status_name(row.new_status).as_bytes(),
            text(&row.classification),
            text(&row.r#type),
            text(&row.component),
            role,
            &guid[..len],
            text(&row.alert_name),
            text(&row.chart_name),
            text(&row.chart_context),
        ]
        .map(|value| if value.is_empty() { &b"unknown"[..] } else { value });
        for (options, value) in self.options.iter_mut().zip(values) {
            if !options.contains_key(value) {
                options.insert(value.to_vec(), 0);
            }
        }
        let selected: [bool; 9] =
            std::array::from_fn(|i| self.patterns[i].as_ref().is_none_or(|pattern| pattern.matches(values[i])));
        let selected_by = selected.iter().filter(|selected| **selected).count();
        if selected_by == 9 {
            // (a GUID's text is ASCII)
            self.keep(row, &String::from_utf8_lossy(&guid[..len]));
        }
        if selected_by >= 8 {
            for ((options, value), selected) in self.options.iter_mut().zip(values).zip(selected) {
                match options.get_mut(value) {
                    // with one facet against the row, only that facet counts it
                    Some(count) if selected_by == 9 || !selected => *count = count.wrapping_add(1),
                    _ => {}
                }
            }
        }
    }

    /// `contexts_v2_alert_transition_keep()`: the place of a row every facet selects. The query hands the rows
    /// out newest first, so the usual path is: the first rows are appended until `last` are held, and every older
    /// one after them is `skips_after`. The search from the last place added, in both directions, is C's; it is
    /// what the `stats` of a `debug` answer count. A row as new as the list's last one is appended to a full list
    /// and falls off it again (a shift), and the search then starts from the head.
    fn keep(&mut self, row: &TransitionRow<'_>, machine_guid: &str) {
        self.matched += 1;
        let global_id = row.global_id as u64;
        if global_id <= self.anchor {
            self.operations.skips_before += 1;
            return;
        }
        if self.kept.is_empty() {
            self.kept.push_back(Kept::of(row, machine_guid));
            self.last_added = 0;
            self.returned += 1;
            self.operations.first += 1;
            return;
        }
        let mut last = self.last_added;
        while last != 0 && global_id > self.kept[last - 1].global_id {
            last -= 1;
            self.operations.backwards += 1;
        }
        while last + 1 < self.kept.len() && global_id < self.kept[last + 1].global_id {
            last += 1;
            self.operations.forwards += 1;
        }
        let full = self.returned >= self.max;
        if full && last + 1 == self.kept.len() && global_id < self.kept[last].global_id {
            self.operations.skips_after += 1;
            return;
        }
        self.returned += 1;
        if global_id > self.kept[last].global_id {
            if self.returned > self.max {
                self.returned -= 1;
                self.operations.shifts += 1;
                self.kept.pop_back();
            }
            // C prepends its "last added" item here, which only on a full list is the row's copy. On a list with
            // room it links an item the list already holds in front of the list, which makes the list a cycle: C
            // then never answers, or answers with rows lost (DEFECTS in the status repository). The row's copy is
            // prepended always. Where C has no answer this one is the port's own: the row goes first, whatever
            // the ids after it.
            self.kept.push_front(Kept::of(row, machine_guid));
            self.last_added = 0;
            self.operations.prepend += 1;
        } else {
            self.kept.push_back(Kept::of(row, machine_guid));
            self.last_added = self.kept.len() - 1;
            self.operations.append += 1;
        }
        while self.returned > self.max {
            self.kept.pop_back();
            if self.last_added >= self.kept.len() {
                self.last_added = 0;
            }
            self.returned -= 1;
            self.operations.shifts += 1;
        }
    }

    /// The rule hashes of the kept rows, each once, in the rows' order: what `configurations` lists.
    pub(super) fn config_hashes(&self) -> Vec<[u8; 16]> {
        let hashes: indexmap::IndexSet<[u8; 16]> = self.kept.iter().map(|kept| kept.config_hash_id).collect();
        hashes.into_iter().collect()
    }

    /// `contexts_v2_alert_transitions_to_json()`: `facets` (not with `mcp`), `transitions`, `configurations` when
    /// `rules` are given (the `config` option: the rules of [`Self::config_hashes`]), `items`, and `stats` with
    /// `debug`. `host` finds the facts of a host by its GUID; `defaults` are localhost's default notification
    /// command and recipient, for a row that has none.
    pub(super) fn to_json(
        &self,
        w: &mut JsonWriter,
        options: u64,
        host: impl Fn(&str) -> Option<HostFacts>,
        defaults: (&[u8], &[u8]),
        rules: Option<&[AlertConfigRow]>,
    ) {
        let (mcp, rfc3339, debug) = (options & MCP != 0, options & RFC3339 != 0, options & DEBUG != 0);
        let k = Keys::with_long(options & JSON_LONG_KEYS != 0);
        let (default_exec, default_recipient) = (c_str(defaults.0), c_str(defaults.1));
        if !mcp {
            w.member_add_array(Some(b"facets"));
            for (i, (id, name, order)) in FACETS.into_iter().enumerate() {
                w.add_array_item_object();
                w.member_add_string("id", id);
                w.member_add_string("name", name);
                w.member_add_uint64("order", order);
                w.member_add_array(Some(b"options"));
                for (value, count) in &self.options[i] {
                    w.add_array_item_object();
                    w.member_add_string("id", value);
                    // a host's option is named by its hostname while the host is known
                    let hostname = (i == NODE).then(|| host(&String::from_utf8_lossy(value))).flatten();
                    match hostname {
                        Some((hostname, _)) => w.member_add_string("name", hostname),
                        None => w.member_add_string("name", value),
                    }
                    w.member_add_uint64("count", u64::from(*count));
                    w.object_close();
                }
                w.array_close();
                w.object_close();
            }
            w.array_close();
        }

        w.member_add_array(Some(b"transitions"));
        for t in &self.kept {
            w.add_array_item_object();
            let facts = host(&t.machine_guid);
            w.member_add_uint64(k.alert_global_id(), t.global_id);
            w.member_add_string_opt("alert", non_empty(&t.alert_name));
            if !mcp {
                w.member_add_uuid("transition_id", &t.transition_id);
                w.member_add_string("machine_guid", &t.machine_guid);
                if let Some((_, node_id)) = facts.as_ref().filter(|(_, node_id)| *node_id != [0; 16]) {
                    w.member_add_uuid("node_id", node_id);
                }
            }
            w.member_add_uuid("config_hash_id", &t.config_hash_id);
            if let Some((hostname, _)) = &facts {
                w.member_add_string("hostname", hostname);
            }
            if mcp {
                w.member_add_string_opt("instance", non_empty(&t.chart_name));
            } else {
                w.member_add_string_opt("instance", non_empty(&t.chart));
                w.member_add_string_opt("instance_n", non_empty(&t.chart_name));
            }
            w.member_add_string_opt("context", non_empty(&t.chart_context));
            w.member_add_string_opt("component", non_empty(&t.component));
            w.member_add_string_opt("classification", non_empty(&t.classification));
            w.member_add_string_opt("type", non_empty(&t.r#type));
            w.member_add_time_t_formatted("when", t.when_key, rfc3339);
            w.member_add_string("info", &t.info);
            w.member_add_string("summary", &t.summary);
            w.member_add_string_opt("units", non_empty(&t.units));

            w.member_add_object("new");
            w.member_add_string("status", status_name(t.new_status));
            w.member_add_double("value", t.new_value);
            w.object_close();

            w.member_add_object("old");
            w.member_add_string("status", status_name(t.old_status));
            w.member_add_double("value", t.old_value);
            w.member_add_time_t("duration", t.duration);
            w.member_add_time_t("raised_duration", t.non_clear_duration);
            w.object_close();

            w.member_add_object("notification");
            w.member_add_time_t_formatted("when", t.exec_run_timestamp, rfc3339);
            w.member_add_time_t("delay", i64::from(t.delay));
            w.member_add_time_t_formatted("delay_up_to_time", t.delay_up_to_timestamp, rfc3339);
            // C hands the 64 bits to a 32-bit parameter
            entry_flags_to_json_array(w, "flags", t.flags as u32);
            w.member_add_string("exec", non_empty(&t.exec).unwrap_or(default_exec));
            // the int goes to an unsigned 64-bit parameter
            w.member_add_uint64("exec_code", i64::from(t.exec_code) as u64);
            w.member_add_string("to", non_empty(&t.recipient).unwrap_or(default_recipient));
            w.object_close();
            w.object_close();
        }
        w.array_close();

        if let Some(rules) = rules {
            let by = ConfigOptions { debug, mcp, rfc3339 };
            w.member_add_array(Some(b"configurations"));
            for rule in rules {
                w.add_array_item_object();
                alert_config_members(w, rule, default_recipient, by);
                w.object_close();
            }
            w.array_close();
        }

        w.member_add_object("items");
        w.member_add_uint64("evaluated", self.evaluated);
        w.member_add_uint64("matched", self.matched);
        w.member_add_uint64("returned", self.returned);
        w.member_add_uint64("max_to_return", self.max);
        w.member_add_uint64("before", self.operations.skips_before);
        w.member_add_uint64("after", self.operations.skips_after + self.operations.shifts);
        w.object_close();

        if debug {
            let o = self.operations;
            w.member_add_object("stats");
            w.member_add_uint64("first", o.first);
            w.member_add_uint64("prepend", o.prepend);
            w.member_add_uint64("append", o.append);
            w.member_add_uint64("backwards", o.backwards);
            w.member_add_uint64("forwards", o.forwards);
            w.member_add_uint64("shifts", o.shifts);
            w.member_add_uint64("skips_before", o.skips_before);
            w.member_add_uint64("skips_after", o.skips_after);
            w.object_close();
        }
    }
}

#[cfg(test)]
mod tests {
    use netdata_agent_text::json::JsonOptions;

    use super::*;

    const T: i64 = 1_700_000_000;
    const HOST_A: [u8; 16] = [0x11; 16];
    const HOST_B: [u8; 16] = [0x22; 16];
    const GUID_A: &str = "11111111-1111-1111-1111-111111111111";
    const GUID_B: &str = "22222222-2222-2222-2222-222222222222";

    fn some(text: &'static str) -> Option<Cow<'static, [u8]>> {
        Some(Cow::Borrowed(text.as_bytes()))
    }

    /// A row with every text set: `cpu_high` of host A going from CLEAR to WARNING, at the global id given.
    fn row(global_id: i64) -> TransitionRow<'static> {
        TransitionRow {
            host_id: HOST_A,
            alarm_id: 7,
            config_hash_id: [0xaa; 16],
            alert_name: some("cpu_high"),
            chart: some("system.cpu"),
            chart_name: some("system.cpu_name"),
            family: some("cpu"),
            recipient: some("sysadmin"),
            units: some("%"),
            exec: some("/bin/notify"),
            chart_context: some("system.cpu"),
            when_key: T + 9,
            duration: 60,
            non_clear_duration: 30,
            // PROCESSED, EXEC_RUN, EXEC_IN_PROGRESS, SAVED, and a bit above the 32 that are printed
            flags: 0x1_1000_0045,
            delay_up_to_timestamp: T + 20,
            info: some("the info"),
            exec_code: 0,
            new_status: 3,
            old_status: 1,
            delay: 5,
            new_value: 91.5,
            old_value: 12.25,
            last_repeat: 0,
            transition_id: [0x71; 16],
            global_id,
            classification: some("Utilization"),
            r#type: some("System"),
            component: some("CPU"),
            exec_run_timestamp: T + 10,
            summary: some("the summary"),
        }
    }

    /// A row with no text at all, of host B, going from WARNING to CRITICAL, whose notifier's exit code is -1.
    fn bare(global_id: i64) -> TransitionRow<'static> {
        TransitionRow {
            host_id: HOST_B,
            alarm_id: 8,
            config_hash_id: [0xbb; 16],
            alert_name: None,
            chart: None,
            chart_name: None,
            family: None,
            recipient: None,
            units: None,
            exec: None,
            chart_context: None,
            when_key: 0,
            duration: 0,
            non_clear_duration: 0,
            flags: 0,
            delay_up_to_timestamp: 0,
            info: None,
            exec_code: -1,
            new_status: 4,
            old_status: 3,
            delay: 0,
            new_value: 0.0,
            old_value: 0.0,
            last_repeat: 0,
            transition_id: [0x72; 16],
            global_id,
            classification: None,
            r#type: None,
            component: None,
            exec_run_timestamp: 0,
            summary: None,
        }
    }

    fn collector(facets: &[(&str, &str)], last: u32, anchor: u64) -> Collector<'static> {
        let mut texts: [Option<Vec<u8>>; 9] = Default::default();
        for (id, text) in facets {
            let at = FACETS.iter().position(|(facet, _, _)| facet == id).expect("a facet's id");
            texts[at] = Some(text.as_bytes().to_vec());
        }
        Collector::new(&texts, b"default-to", last, anchor)
    }

    /// The global ids kept, the six numbers of `items`, and the eight of `stats`.
    fn kept_of(c: &Collector<'_>) -> (Vec<u64>, [u64; 6], [u64; 8]) {
        let o = c.operations;
        let items = [c.evaluated, c.matched, c.returned, c.max, o.skips_before, o.skips_after + o.shifts];
        let stats =
            [o.first, o.prepend, o.append, o.backwards, o.forwards, o.shifts, o.skips_before, o.skips_after];
        (c.kept.iter().map(|kept| kept.global_id).collect(), items, stats)
    }

    fn after(last: u32, anchor: u64, global_ids: &[i64]) -> (Vec<u64>, [u64; 6], [u64; 8]) {
        let mut c = collector(&[], last, anchor);
        for &global_id in global_ids {
            c.row(&row(global_id));
        }
        kept_of(&c)
    }

    /// `contexts_v2_alert_transition_keep()` over rows that come newest first, as the window's statement hands them
    /// out: the first `last` rows above the anchor are kept, every older one after them is counted as after, every
    /// row at or below the anchor as before (and still as matched).
    #[test]
    fn the_newest_rows_after_the_anchor_are_kept() {
        let newest_first = [9, 8, 7, 6, 5, 4, 3, 2, 1];
        // everything fits
        let all: Vec<u64> = (1..=9).rev().collect();
        assert_eq!(after(200, 0, &newest_first), (all, [9, 9, 9, 200, 0, 0], [1, 0, 8, 0, 0, 0, 0, 0]));
        // four of them, above the third
        assert_eq!(after(4, 3, &newest_first), (vec![9, 8, 7, 6], [9, 9, 4, 4, 3, 2], [1, 0, 3, 0, 0, 0, 3, 2]));
        // one row, one place
        assert_eq!(after(1, 0, &[5]), (vec![5], [1, 1, 1, 1, 0, 0], [1, 0, 0, 0, 0, 0, 0, 0]));
        // the anchor itself is before, and nothing is kept
        assert_eq!(after(4, 9, &newest_first), (vec![], [9, 9, 0, 4, 9, 0], [0, 0, 0, 0, 0, 0, 9, 0]));
        // no row at all
        assert_eq!(after(1, 0, &[]), (vec![], [0, 0, 0, 1, 0, 0], [0; 8]));
    }

    /// Rows as new as the list's last one at a full list: each is appended and falls off again, which counts as a
    /// shift (so as "after"), and the search for the next row's place starts from the head and walks forward.
    #[test]
    fn a_tie_at_a_full_list_is_a_shift() {
        // 9 and the first 8 are kept; the second and the third 8 are each appended and dropped; 7 walks from the
        // head to the tail and is skipped
        let (kept, items, stats) = after(2, 0, &[9, 8, 8, 8, 7]);
        assert_eq!(kept, [9, 8]);
        assert_eq!(items, [5, 5, 2, 2, 0, 3]);
        assert_eq!(stats, [1, 0, 3, 0, 1, 2, 0, 1]);
        // with room for them the ties are simply kept
        let roomy = after(5, 0, &[9, 8, 8, 8, 7]);
        assert_eq!(roomy, (vec![9, 8, 8, 8, 7], [5, 5, 5, 5, 0, 0], [1, 0, 4, 0, 0, 0, 0, 0]));
    }

    /// A row newer than the list's (the statement by id has no order): on a full list it takes the place of the
    /// oldest; on a list with room it goes first (C has no answer there: its list becomes a cycle, see `keep`).
    #[test]
    fn a_newer_row_goes_first() {
        assert_eq!(after(1, 0, &[5, 9]), (vec![9], [2, 2, 1, 1, 0, 1], [1, 1, 0, 0, 0, 1, 0, 0]));
        assert_eq!(after(3, 0, &[5, 9]), (vec![9, 5], [2, 2, 2, 3, 0, 0], [1, 1, 0, 0, 0, 0, 0, 0]));
    }

    /// A row that is newer than the place last added to (rows read by a transition's id come in no order): the
    /// search walks backwards from that place, and on a full list the oldest row falls off and the new one goes
    /// to the head, not to its place by id. C's results, walked by hand.
    #[test]
    fn a_row_out_of_order_is_searched_backwards_and_goes_first() {
        // 8 walks back from 5 past 7; the list is full, 5 falls off, 8 is the new head
        assert_eq!(after(3, 0, &[9, 7, 5, 8]), (vec![8, 9, 7], [4, 4, 3, 3, 0, 1], [1, 1, 2, 1, 0, 1, 0, 0]));
        // a full list of two: the place last added to is the tail and the row before it is newer, so no step back
        assert_eq!(after(2, 0, &[9, 7, 8]), (vec![8, 9], [3, 3, 2, 2, 0, 1], [1, 1, 1, 0, 0, 1, 0, 0]));
    }

    /// Two rows with one global id at a full list: the second is appended and falls off, so the first to arrive
    /// is the one that stays.
    #[test]
    fn of_two_rows_with_one_id_the_first_to_arrive_stays() {
        let mut c = collector(&[], 2, 0);
        for row in [row(9), row(8), bare(8)] {
            c.row(&row);
        }
        let kept: Vec<_> = c.kept.iter().map(|kept| (kept.global_id, kept.transition_id)).collect();
        assert_eq!(kept, [(9, [0x71; 16]), (8, [0x71; 16])]);
        assert_eq!(kept_of(&c).2, [1, 0, 2, 0, 0, 1, 0, 0]);
    }

    /// The rule hashes the answer's `configurations` asks for: the kept rows', each once, in the rows' order.
    #[test]
    fn the_kept_rows_rule_hashes_are_listed_once_in_their_order() {
        let mut c = collector(&[], 10, 0);
        for row in [bare(9), row(8), bare(7), row(6)] {
            c.row(&row);
        }
        assert_eq!(c.config_hashes(), [[0xbb; 16], [0xaa; 16]]);
        assert!(collector(&[], 10, 0).config_hashes().is_empty());
    }

    /// The facets: every value a row shows is an option of its facet, in first-seen order, `unknown` for a text
    /// the row lacks and localhost's default recipient for its role. A row every facet selects is kept and counts
    /// everywhere; a row one facet rejects counts on that facet alone; a row two reject counts nowhere. A
    /// pattern is exact, whatever the case, with the web's separators.
    #[test]
    fn a_row_counts_on_the_facets_that_would_keep_it() {
        let with = |status: i32, host: [u8; 16], alert: &'static str| TransitionRow {
            new_status: status,
            host_id: host,
            alert_name: some(alert),
            ..row(1)
        };
        let mut c = collector(&[("f_status", "warning|nothing"), ("f_alert", "A1")], 200, 0);
        c.row(&with(3, HOST_A, "a1")); // kept
        c.row(&with(4, HOST_A, "a2")); // two facets against it
        c.row(&with(3, HOST_B, "a1")); // kept
        c.row(&with(4, HOST_A, "a1")); // its status alone against it
        c.row(&with(3, HOST_A, "a2")); // its name alone against it
        c.row(&bare(1)); // CRITICAL and no name: two against it
        let options = |id: &str| {
            let at = FACETS.iter().position(|(facet, _, _)| *facet == id).unwrap();
            let named = |(value, count): (&Vec<u8>, &u32)| (String::from_utf8_lossy(value).into_owned(), *count);
            c.options[at].iter().map(named).collect::<Vec<_>>()
        };
        let counted =
            |pairs: &[(&str, u32)]| pairs.iter().map(|(name, n)| ((*name).to_owned(), *n)).collect::<Vec<_>>();
        assert_eq!(options("f_status"), counted(&[("WARNING", 2), ("CRITICAL", 1)]));
        assert_eq!(options("f_alert"), counted(&[("a1", 2), ("a2", 1), ("unknown", 0)]));
        assert_eq!(options("f_node"), counted(&[(GUID_A, 1), (GUID_B, 1)]));
        assert_eq!(options("f_role"), counted(&[("sysadmin", 2), ("default-to", 0)]));
        assert_eq!(options("f_class"), counted(&[("Utilization", 2), ("unknown", 0)]));
        assert_eq!(options("f_type"), counted(&[("System", 2), ("unknown", 0)]));
        assert_eq!(options("f_component"), counted(&[("CPU", 2), ("unknown", 0)]));
        assert_eq!(options("f_instance"), counted(&[("system.cpu_name", 2), ("unknown", 0)]));
        assert_eq!(options("f_context"), counted(&[("system.cpu", 2), ("unknown", 0)]));
        assert_eq!((c.evaluated, c.matched, c.returned), (6, 2, 2));

        // a text with no word in it is no pattern: every row is selected
        let mut c = collector(&[("f_status", "|"), ("f_alert", ",")], 200, 0);
        c.row(&with(4, HOST_A, "a2"));
        assert_eq!((c.evaluated, c.matched), (1, 1));
        // a pattern is whole: a part of a value selects nothing
        let mut c = collector(&[("f_alert", "a")], 200, 0);
        c.row(&with(3, HOST_A, "a1"));
        assert_eq!((c.evaluated, c.matched), (1, 0));
    }

    /// A kept row is a copy with C's byte cuts: 47 bytes for the alert's name, its units, class, type and
    /// component, 95 for the context and the recipient, 511 for the command, the info and the summary, 1199 for the
    /// chart's id and name; a text ends at its terminator. A chart without a stored name is named by its id.
    #[test]
    fn a_kept_row_s_texts_are_cut_as_c_cuts_them() {
        let long = |n: usize| -> Option<Cow<'static, [u8]>> { Some(Cow::Owned(vec![b'x'; n])) };
        let wide = TransitionRow {
            alert_name: long(60),
            units: long(48),
            classification: long(47),
            r#type: long(100),
            component: long(1),
            chart_context: long(96),
            recipient: long(95),
            exec: long(600),
            info: long(512),
            summary: long(511),
            chart: long(1300),
            chart_name: None,
            ..row(1)
        };
        let kept = Kept::of(&wide, GUID_A);
        let lengths = [
            kept.alert_name.len(),
            kept.units.len(),
            kept.classification.len(),
            kept.r#type.len(),
            kept.component.len(),
            kept.chart_context.len(),
            kept.recipient.len(),
            kept.exec.len(),
            kept.info.len(),
            kept.summary.len(),
            kept.chart.len(),
            kept.chart_name.len(),
        ];
        assert_eq!(lengths, [47, 47, 47, 47, 1, 95, 95, 511, 511, 511, 1199, 1199]);
        // a stored name that is empty stays empty; a text ends at its terminator
        let odd = TransitionRow { chart_name: some(""), info: some("seen\0hidden"), ..row(1) };
        let kept = Kept::of(&odd, GUID_A);
        assert_eq!((kept.chart_name.as_slice(), kept.info.as_slice()), (&b""[..], &b"seen"[..]));
        // 64 bits of flags are kept; a negative global id reads as C's unsigned one
        let kept = Kept::of(&TransitionRow { global_id: -1, ..row(1) }, GUID_A);
        assert_eq!((kept.flags, kept.global_id), (0x1_1000_0045, u64::MAX));
    }

    /// What `write` prints into an open object, minified, without the object's braces.
    fn members(write: impl FnOnce(&mut JsonWriter)) -> String {
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        write(&mut w);
        w.finalize();
        let text = String::from_utf8(w.into_bytes()).unwrap();
        text[1..text.len() - 1].to_owned()
    }

    /// Host A is known, with a node id; host B is not.
    fn host(guid: &str) -> Option<HostFacts> {
        (guid == GUID_A).then(|| ("box".to_owned(), [0x99; 16]))
    }

    fn printed(c: &Collector<'_>, options: u64, rules: Option<&[AlertConfigRow]>) -> String {
        members(|w| c.to_json(w, options, host, (&b"/default/exec"[..], &b"default-to"[..]), rules))
    }

    /// `contexts_v2_alert_transitions_to_json()`: the facets in the enum's order with their options (a host's
    /// named by its hostname while it is known), each transition's members in C's order with C's nulls, empty
    /// texts and defaults, then `items`.
    #[test]
    fn the_answer_is_facets_transitions_and_items() {
        let mut c = collector(&[], 200, 0);
        c.row(&row(9));
        c.row(&bare(8));
        let text = printed(&c, 0, None);
        let facets = concat!(
            r#""facets":[{"id":"f_status","name":"Alert Status","order":1,"options":["#,
            r#"{"id":"WARNING","name":"WARNING","count":1},{"id":"CRITICAL","name":"CRITICAL","count":1}]},"#,
            r#"{"id":"f_class","name":"Alert Class","order":4,"options":["#,
            r#"{"id":"Utilization","name":"Utilization","count":1},{"id":"unknown","name":"unknown","count":1}]},"#,
            r#"{"id":"f_type","name":"Alert Type","order":2,"options":["#,
        );
        assert!(text.starts_with(facets), "{text}");
        let node = concat!(
            r#"{"id":"f_node","name":"Alert Node","order":6,"options":["#,
            r#"{"id":"11111111-1111-1111-1111-111111111111","name":"box","count":1},"#,
            r#"{"id":"22222222-2222-2222-2222-222222222222","name":"22222222-2222-2222-2222-222222222222","#,
            r#""count":1}]},"#,
            r#"{"id":"f_alert","name":"Alert Name","order":7,"options":["#,
        );
        assert!(text.contains(node), "{text}");
        let ids: Vec<&str> = text.match_indices(r#"{"id":"f_"#).map(|(at, _)| &text[at + 7..at + 13]).collect();
        assert_eq!(ids, ["f_stat", "f_clas", "f_type", "f_comp", "f_role", "f_node", "f_aler", "f_inst", "f_cont"]);

        let new = members(|w| w.member_add_double("value", 91.5));
        let old = members(|w| w.member_add_double("value", 12.25));
        let full = format!(
            concat!(
                r#""transitions":[{{"gi":9,"alert":"cpu_high","transition_id":"71717171-7171-7171-7171-717171717171","#,
                r#""machine_guid":"11111111-1111-1111-1111-111111111111","#,
                r#""node_id":"99999999-9999-9999-9999-999999999999","#,
                r#""config_hash_id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","hostname":"box","instance":"system.cpu","#,
                r#""instance_n":"system.cpu_name","context":"system.cpu","component":"CPU","#,
                r#""classification":"Utilization","type":"System","when":1700000009,"info":"the info","#,
                r#""summary":"the summary","units":"%","new":{{"status":"WARNING",{new}}},"#,
                r#""old":{{"status":"CLEAR",{old},"duration":60,"raised_duration":30}},"#,
                r#""notification":{{"when":1700000010,"delay":5,"delay_up_to_time":1700000020,"#,
                r#""flags":["PROCESSED","EXEC_RUN","EXEC_IN_PROGRESS","SAVED"],"exec":"/bin/notify","exec_code":0,"#,
                r#""to":"sysadmin"}}}},"#,
            ),
            new = new,
            old = old
        );
        assert!(text.contains(&full), "{text}");
        // no text at all: nulls, two empty texts, localhost's defaults; a host that is not known has neither a
        // name nor a node id; the exit code -1 through the unsigned writer
        let zero = members(|w| w.member_add_double("value", 0.0));
        let empty = format!(
            concat!(
                r#"{{"gi":8,"alert":null,"transition_id":"72727272-7272-7272-7272-727272727272","#,
                r#""machine_guid":"22222222-2222-2222-2222-222222222222","#,
                r#""config_hash_id":"bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb","instance":null,"instance_n":null,"#,
                r#""context":null,"component":null,"classification":null,"type":null,"when":0,"info":"","#,
                r#""summary":"","units":null,"new":{{"status":"CRITICAL",{zero}}},"#,
                r#""old":{{"status":"WARNING",{zero},"duration":0,"raised_duration":0}},"#,
                r#""notification":{{"when":0,"delay":0,"delay_up_to_time":0,"flags":[],"exec":"/default/exec","#,
                r#""exec_code":18446744073709551615,"to":"default-to"}}}}],"#,
                r#""items":{{"evaluated":2,"matched":2,"returned":2,"max_to_return":200,"before":0,"after":0}}"#,
            ),
            zero = zero
        );
        assert!(text.ends_with(&empty), "{text}");
    }

    /// The options of the answer: `mcp` drops the facets and a transition's ids and second instance name, and
    /// names the instance by the chart's name; `rfc3339` sends the three times through the time writer, where 0
    /// is null; long keys spell `global_id`; `debug` adds the keep's `stats`; rules given are `configurations`.
    #[test]
    fn the_answer_follows_the_request_s_options() {
        let mut c = collector(&[], 1, 0);
        c.row(&row(9));
        c.row(&bare(8));
        let text = printed(&c, MCP, None);
        let head = concat!(
            r#""transitions":[{"gi":9,"alert":"cpu_high","config_hash_id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","#,
            r#""hostname":"box","instance":"system.cpu_name","context":"system.cpu","#,
        );
        assert!(text.starts_with(head), "{text}");
        let items = r#""items":{"evaluated":2,"matched":2,"returned":1,"max_to_return":1,"before":0,"after":1}"#;
        assert!(text.ends_with(items), "{text}");

        let text = printed(&c, RFC3339, None);
        assert!(text.contains(r#""type":"System","when":"20"#) && !text.contains("1700000009"), "{text}");
        assert!(text.contains(r#""notification":{"when":"20"#) && text.contains(r#""delay":5,"delay_up_to_time":"20"#));
        let mut zeros = collector(&[], 1, 0);
        zeros.row(&bare(8));
        let text = printed(&zeros, RFC3339, None);
        assert!(text.contains(r#""type":null,"when":null,"info":"""#), "{text}");
        assert!(text.contains(r#""notification":{"when":null,"delay":0,"delay_up_to_time":null,"flags":[]"#), "{text}");

        assert!(printed(&c, JSON_LONG_KEYS, None).contains(r#""transitions":[{"global_id":9,"alert":"cpu_high""#));

        let text = printed(&c, DEBUG, None);
        let stats = concat!(
            r#""items":{"evaluated":2,"matched":2,"returned":1,"max_to_return":1,"before":0,"after":1},"#,
            r#""stats":{"first":1,"prepend":0,"append":0,"backwards":0,"forwards":0,"shifts":0,"skips_before":0,"#,
            r#""skips_after":1}"#
        );
        assert!(text.ends_with(stats), "{text}");

        // the rules of the kept rows, each hash once, in the rows' order
        let mut twice = collector(&[], 200, 0);
        for (global_id, hash) in [(9, 0xaa_u8), (8, 0xbb), (7, 0xaa)] {
            twice.row(&TransitionRow { config_hash_id: [hash; 16], ..row(global_id) });
        }
        assert_eq!(twice.config_hashes(), [[0xaa; 16], [0xbb; 16]]);
        assert!(!printed(&twice, 0, None).contains("configurations"));
        assert!(printed(&twice, 0, Some(&[])).contains(r#"}],"configurations":[],"items":{"#));
    }
}
