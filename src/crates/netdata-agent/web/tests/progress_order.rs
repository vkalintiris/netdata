//! The progress table's row order against C's (`tests/oracle/gen-progress.c`): C's script of starts and finishes
//! replayed on the table, then its rows compared with the order `progress_function_result()` listed, after a churn
//! that leaves tombstones in the index, after the index grows, and through a long run of unique ids whose tombstones
//! make C rebuild the index at its size.

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

/// The generator's `tx_of()`: two 64-bit mixes of `n`, in memory order.
fn tx_of(n: u64) -> Transaction {
    let a = n.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1);
    let b = (!n).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let mut tx = [0; 16];
    tx[..8].copy_from_slice(&a.to_le_bytes());
    tx[8..].copy_from_slice(&b.to_le_bytes());
    tx
}

#[test]
fn progress_rows_follow_cs_slots() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors/progress_order.txt");
    let script = std::fs::read_to_string(path).unwrap();
    let table = Table::with_clock(clock);
    let (mut t, mut renders) = (1_000_000u64, 0);
    let start = |tx: &Transaction, t: u64| {
        table.start(tx, t, Start { mode: Some(Mode::Get), acl: 0, query: b"q", client: b"c" });
    };
    let finish = |tx: &Transaction, t: u64, code| {
        table.finished(tx, t, Finish { code, duration_ut: 10, response_size: 1, sent_size: 1 });
    };
    let mut render = |line: Option<&str>| {
        let expected: Vec<&str> = line.and_then(|l| l.strip_prefix("R ")).expect("an R line").split(' ').collect();
        let actual: Vec<String> = table.visit(|_, rows| rows.map(|(tx, _)| hex(tx)).collect());
        assert_eq!(actual, expected, "render {renders}");
        renders += 1;
    };
    let mut lines = script.lines().filter(|l| !l.starts_with('#'));
    while let Some(line) = lines.next() {
        let mut words = line.split(' ');
        match words.next() {
            Some("S") => {
                t += 1;
                start(&unhex(words.next().unwrap()), t);
            }
            Some("F") => {
                t += 1;
                let tx = unhex(words.next().unwrap());
                finish(&tx, t, words.next().unwrap().parse().unwrap());
            }
            Some("R") => render(Some(line)),
            Some("L") => {
                let n: Vec<u64> = words.map(|w| w.parse().unwrap()).collect();
                let [first, count, every] = n[..] else { panic!("bad line {line:?}") };
                for i in 0..count {
                    let tx = tx_of(first + i);
                    start(&tx, t + 1);
                    finish(&tx, t + 2, 200);
                    t += 2;
                    if i % every == every - 1 {
                        render(lines.next());
                    }
                }
            }
            other => panic!("bad line {other:?}"),
        }
    }
    assert_eq!(renders, 7);
}
