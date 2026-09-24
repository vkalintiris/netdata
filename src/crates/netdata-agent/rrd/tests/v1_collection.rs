//! The data-collection unit tests of the C agent (`run_test()` in `src/daemon/unit_test.c`), replayed through
//! `collection::timed_done()`: each case feeds values at given microsecond intervals and checks the stored ring.

mod v1_cases;

use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType, Charts};
use netdata_agent_rrd::collection::{now_realtime_timeval, set_value, timed_done};
use netdata_agent_rrd::mode::DbMode;
use netdata_agent_storage::storage_number::{SN_DEFAULT_FLAGS, pack, unpack};

/// `[db] gap when lost iterations above` after `netdata_conf_section_db()`: 1 + 2.
const GAP_WHEN_LOST_ITERATIONS_ABOVE: i64 = 3;

/// `roundndd(v * 10000000.0)`: the C comparison.
fn same(a: f64, b: f64) -> bool {
    (a * 10_000_000.0).round() == (b * 10_000_000.0).round() || (a.is_nan() && b.is_nan())
}

#[test]
fn c_unit_tests() {
    let mut failures = Vec::new();
    for case in v1_cases::CASES {
        let charts = Charts::default();
        let name = format!("unittest-{}", case.name);
        let (st, _) = charts.create(&ChartSpec {
            type_: "netdata",
            id: &name,
            name: Some(&name),
            family: Some("netdata"),
            context: None,
            title: "Unit Testing",
            units: "a value",
            plugin: "unittest",
            module: None,
            priority: 1,
            update_every: case.update_every,
            chart_type: ChartType::Line,
            mode: DbMode::Ram,
            history_entries: 3600,
            page_size: 4096,
        });
        let algorithm = Algorithm::from_name(case.algorithm);
        let (rd, _) = st.dim_add("dim1", None, case.multiplier, case.divisor, algorithm);
        let rd2 = (!case.feed2.is_empty()).then(|| {
            st.dim_add("dim2", None, case.multiplier, case.divisor, algorithm)
                .0
        });
        for (c, &(microseconds, value)) in case.feed.iter().enumerate() {
            if c > 0 {
                st.update_collection(|x| x.usec_since_last_update = microseconds);
            }
            let t = now_realtime_timeval();
            set_value(&rd, t, value);
            if let Some(rd2) = &rd2 {
                set_value(rd2, t, case.feed2[c]);
            }
            timed_done(&st, t, false, GAP_WHEN_LOST_ITERATIONS_ABOVE);
            if c == 0 {
                // run_test() pins the first collection `microseconds` past the second boundary.
                let usec = microseconds as i64;
                rd.update_collection(|d| d.last_collected_time.1 = usec);
                st.update_collection(|x| {
                    x.last_collected.1 = usec;
                    x.last_updated.1 = usec;
                });
            }
        }
        let counter = st.collection().counter;
        if counter != case.results.len() {
            failures.push(format!(
                "{}: stored {counter} entries, expected {}",
                case.name,
                case.results.len()
            ));
        }
        for c in 0..counter.min(case.results.len()) {
            let v = unpack(rd.ring().unwrap().slot(c));
            let n = unpack(pack(case.results[c], SN_DEFAULT_FLAGS));
            if !same(v, n) {
                failures.push(format!(
                    "{}/dim1 position {}: expected {n}, found {v}",
                    case.name,
                    c + 1
                ));
            }
            if let Some(rd2) = &rd2 {
                let v = unpack(rd2.ring().unwrap().slot(c));
                let n = case.results2[c];
                if !same(v, n) {
                    failures.push(format!(
                        "{}/dim2 position {}: expected {n}, found {v}",
                        case.name,
                        c + 1
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
