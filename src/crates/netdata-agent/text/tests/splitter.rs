//! `quoted_strings_splitter()` against the C golden vectors.

mod common;

use common::check;
use netdata_agent_text::line_splitter::{Separators, quoted_strings_splitter};

fn separators(name: &str) -> Separators {
    match name {
        "whitespace" => Separators::Whitespace,
        "pluginsd" => Separators::Pluginsd,
        "config" => Separators::Config,
        "group_by_label" => Separators::GroupByLabel,
        "dyncfg_id" => Separators::DyncfgId,
        other => panic!("unknown map {other}"),
    }
}

#[test]
fn splitter_matches_c() {
    let n = check(
        "splitter.tsv",
        |row| (row.num::<usize>(3), row.bytes(4).to_vec()),
        |row| {
            let words = quoted_strings_splitter(row.bytes(1), row.num(2), separators(row.str(0)));
            (words.len(), words.join(&[0x1fu8][..]))
        },
    );
    assert!(n > 20_000);
}
