//! Replays every `tests/scripts/*.script` against the Rust port and compares the output byte for byte with
//! `tests/expected/*.out`, which `tests/oracle/gen-expected.sh` produced by running the same scripts through the C
//! inicfg (`tests/oracle/inicfg-oracle.c`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use netdata_agent_inicfg::Config;

fn opt(field: &str) -> Option<&str> {
    (field != "\\N").then_some(field)
}

fn text(value: Option<Vec<u8>>) -> String {
    value.map_or_else(
        || "(null)".to_string(),
        |v| String::from_utf8_lossy(&v).into_owned(),
    )
}

fn bits(field: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(field, 16).expect("double bits"))
}

fn run(script: &str, fixtures: &Path) -> String {
    let mut cfg = Config::new();
    let mut out = String::new();
    for line in script.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let g = |i: usize| f.get(i).copied().unwrap_or("");
        let num = |i: usize| g(i).parse::<i64>().expect("number");
        let unum = |i: usize| g(i).parse::<u64>().expect("unsigned number");
        write!(out, "{} -> ", f.join("\t")).unwrap();
        let result = match g(0) {
            "load" => {
                let loaded = cfg
                    .load(&fixtures.join(g(1)), g(2) == "1", opt(g(3)))
                    .is_ok();
                u8::from(loaded).to_string()
            }
            "get" => text(cfg.get(g(1), g(2), opt(g(3)))),
            "set" => text(Some(cfg.set(g(1), g(2), g(3)))),
            "get_filename" => text(cfg.get_filename(g(1), g(2), opt(g(3)))),
            "get_path" => text(cfg.get_path(g(1), g(2), opt(g(3)))),
            "get_boolean" => u8::from(cfg.get_boolean(g(1), g(2), g(3) != "0")).to_string(),
            "get_boolean_ondemand" => cfg
                .get_boolean_ondemand(g(1), g(2), num(3) as i32)
                .to_string(),
            "set_boolean" => u8::from(cfg.set_boolean(g(1), g(2), g(3) != "0")).to_string(),
            "get_number" => cfg.get_number(g(1), g(2), num(3)).to_string(),
            "get_number_range" => cfg
                .get_number_range(g(1), g(2), num(3), num(4), num(5))
                .to_string(),
            "set_number" => cfg.set_number(g(1), g(2), num(3)).to_string(),
            "get_double" => format!("{:016x}", cfg.get_double(g(1), g(2), bits(g(3))).to_bits()),
            "set_double" => format!("{:016x}", cfg.set_double(g(1), g(2), bits(g(3))).to_bits()),
            "get_duration_seconds" => cfg.get_duration_seconds(g(1), g(2), num(3)).to_string(),
            "set_duration_seconds" => cfg.set_duration_seconds(g(1), g(2), num(3)).to_string(),
            "get_duration_ms" => cfg.get_duration_ms(g(1), g(2), unum(3)).to_string(),
            "set_duration_ms" => cfg.set_duration_ms(g(1), g(2), unum(3)).to_string(),
            "get_duration_days_to_seconds" => cfg
                .get_duration_days_to_seconds(g(1), g(2), unum(3) as u32)
                .to_string(),
            "get_size_bytes" => cfg.get_size_bytes(g(1), g(2), unum(3)).to_string(),
            "set_size_bytes" => cfg.set_size_bytes(g(1), g(2), unum(3)).to_string(),
            "get_size_mb" => cfg.get_size_mb(g(1), g(2), unum(3)).to_string(),
            "set_size_mb" => cfg.set_size_mb(g(1), g(2), unum(3)).to_string(),
            "exists" => u8::from(cfg.exists(g(1), g(2))).to_string(),
            "set_default_raw_value" => {
                cfg.set_default_raw_value(g(1), g(2), g(3));
                "ok".to_string()
            }
            "move" => u8::from(cfg.move_option(g(1), g(2), g(3), g(4))).to_string(),
            "move_everywhere" => u8::from(cfg.move_everywhere(g(1), g(2))).to_string(),
            "destroy_section" => {
                cfg.section_destroy_non_loaded(g(1));
                "ok".to_string()
            }
            "destroy_option" => {
                cfg.section_option_destroy_non_loaded(g(1), g(2));
                "ok".to_string()
            }
            "stream_needs_dbengine" => u8::from(cfg.stream_conf_needs_dbengine()).to_string(),
            "stream_has_api" => u8::from(cfg.stream_conf_has_api_enabled()).to_string(),
            "generate" => {
                let generated = cfg.generate(g(1) == "1", g(2) == "1");
                format!("\n{}<<<END", String::from_utf8_lossy(&generated))
            }
            other => panic!("unknown step {other}"),
        };
        writeln!(out, "{result}").unwrap();
    }
    out
}

#[test]
fn scripts_match_the_c_implementation() {
    let tests: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests"].iter().collect();
    let mut scripts: Vec<PathBuf> = std::fs::read_dir(tests.join("scripts"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "script"))
        .collect();
    scripts.sort();
    assert!(!scripts.is_empty());
    let mut failures = Vec::new();
    for script in &scripts {
        let name = script.file_stem().unwrap().to_string_lossy().into_owned();
        let expected =
            std::fs::read_to_string(tests.join("expected").join(format!("{name}.out"))).unwrap();
        let got = run(
            &std::fs::read_to_string(script).unwrap(),
            &tests.join("fixtures"),
        );
        if got != expected {
            let first = got
                .lines()
                .zip(expected.lines())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| got.lines().count().min(expected.lines().count()));
            failures.push(format!(
                "{name}: first difference at line {}\n  rust: {:?}\n  c:    {:?}",
                first + 1,
                got.lines().nth(first),
                expected.lines().nth(first)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
