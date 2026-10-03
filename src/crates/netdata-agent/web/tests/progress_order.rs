//! The progress table's row order against C's (`tests/oracle/gen-progress.c`): C's script of starts and finishes
//! replayed on the table, then its rows compared with the order `progress_function_result()` listed, after a churn
//! that leaves tombstones in the index and after the index grows.

use netdata_agent_web::progress::{Finish, Start, Table, Transaction};
use netdata_agent_web::request::Mode;

fn clock() -> u64 {
    5_000_000
}

fn unhex(s: &str) -> Transaction {
    let mut tx = [0; 16];
    for (i, b) in tx.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
    }
    tx
}

fn hex(tx: &Transaction) -> String {
    tx.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn progress_rows_follow_cs_slots() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors/progress_order.txt");
    let script = std::fs::read_to_string(path).unwrap();
    let table = Table::with_clock(clock);
    let (mut t, mut renders) = (1_000_000u64, 0);
    for line in script.lines().filter(|l| !l.starts_with('#')) {
        let mut words = line.split(' ');
        match words.next() {
            Some("S") => {
                t += 1;
                let start = Start { mode: Some(Mode::Get), acl: 0, query: b"q", client: b"c" };
                table.start(&unhex(words.next().unwrap()), t, start);
            }
            Some("F") => {
                t += 1;
                let tx = unhex(words.next().unwrap());
                let code = words.next().unwrap().parse().unwrap();
                table.finished(&tx, t, Finish { code, duration_ut: 10, response_size: 1, sent_size: 1 });
            }
            Some("R") => {
                let expected: Vec<&str> = words.collect();
                let actual: Vec<String> = table.visit(|_, rows| rows.map(|(tx, _)| hex(tx)).collect());
                assert_eq!(actual, expected, "render {renders}");
                renders += 1;
            }
            other => panic!("bad line {other:?}"),
        }
    }
    assert_eq!(renders, 2);
}
