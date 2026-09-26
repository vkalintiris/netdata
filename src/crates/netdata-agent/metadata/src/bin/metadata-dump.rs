//! `metadata-dump <database> [--table <name>]... [--mask <table>.<column>]... [--sort <table>]...
//! [--alias <table>.<column>]... [--skip-host-charts <hex>]...` prints a netdata SQLite database in a canonical form
//! the parity checks compare: the file header's fields, the pragmas the agents set, `sqlite_master` in rowid order,
//! then each named table's rows in rowid order, blobs as hex, `date_created` and the named columns masked.
//!
//! - `--sort` prints a table's rows sorted instead: C writes labels in pointer order, which differs run to run.
//! - `--alias` prints the 16-byte blobs of a column as `u<N>`, numbered by first appearance across every aliased
//!   column: random UUIDs then compare, and stay joinable between tables.
//! - `--skip-host-charts` leaves out a host's chart rows and the dimension and chart label rows of those charts.
//!
//! `--exec <sql>` runs statements on the database instead and prints nothing: the checks build old and corrupted
//! databases with it. The database is opened read-only unless it runs statements.

use std::process::ExitCode;

use std::collections::HashMap;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

fn usage() -> ExitCode {
    eprintln!(
        "usage: metadata-dump <database> [--table <name>]... [--mask <table>.<column>]... [--sort <table>]... \
         [--alias <table>.<column>]... [--skip-host-charts <hex>]...\n       \
         metadata-dump <database> --exec <sql>"
    );
    ExitCode::from(2)
}

fn value(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Null => "NULL".into(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(r) => format!("{r:?}"),
        ValueRef::Text(t) => format!("{:?}", String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => format!(
            "x'{}'",
            b.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ),
    }
}

fn header(path: &str) -> std::io::Result<String> {
    let mut h = [0u8; 100];
    std::io::Read::read_exact(&mut std::fs::File::open(path)?, &mut h)?;
    let be16 = |o: usize| u16::from_be_bytes([h[o], h[o + 1]]);
    let be32 = |o: usize| u32::from_be_bytes([h[o], h[o + 1], h[o + 2], h[o + 3]]);
    Ok(format!(
        "header page_size={} write_version={} read_version={} schema_format={} encoding={} user_version={} \
         incremental_vacuum={} application_id={} sqlite_version={}",
        be16(16),
        h[18],
        h[19],
        be32(44),
        be32(56),
        be32(60),
        be32(64),
        be32(68),
        be32(96)
    ))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        return usage();
    };
    if args.get(1).map(String::as_str) == Some("--exec") {
        let Some(sql) = args.get(2) else {
            return usage();
        };
        return match Connection::open(path).and_then(|c| c.execute_batch(sql)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{path}: {err}");
                ExitCode::from(1)
            }
        };
    }
    let (mut tables, mut masks) = (Vec::new(), vec!["date_created".to_string()]);
    let (mut sorted, mut aliased, mut skip_hosts) = (Vec::new(), Vec::new(), Vec::new());
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match (arg.as_str(), rest.next()) {
            ("--table", Some(t)) => tables.push(t.clone()),
            ("--mask", Some(m)) => masks.push(m.clone()),
            ("--sort", Some(t)) => sorted.push(t.clone()),
            ("--alias", Some(c)) => aliased.push(c.clone()),
            ("--skip-host-charts", Some(h))
                if h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                skip_hosts.push(h.to_ascii_lowercase())
            }
            _ => return usage(),
        }
    }
    let mut aliases: HashMap<Vec<u8>, usize> = HashMap::new();
    let mut run = || -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", header(path)?);
        let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        for pragma in [
            "auto_vacuum",
            "journal_mode",
            "page_size",
            "encoding",
            "user_version",
            "application_id",
        ] {
            let v: String = c.query_row(&format!("PRAGMA {pragma}"), [], |r| {
                Ok(value(r.get_ref(0)?))
            })?;
            println!("pragma {pragma}={v}");
        }
        let mut stmt =
            c.prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY rowid")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            println!(
                "schema {} {} {} {}",
                value(r.get_ref(0)?),
                value(r.get_ref(1)?),
                value(r.get_ref(2)?),
                value(r.get_ref(3)?)
            );
        }
        for table in &tables {
            let exists: i64 = c.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |r| r.get(0),
            )?;
            if exists == 0 {
                println!("missing {table}");
                continue;
            }
            // a skipped host's charts, and what hangs off them
            let skipped: Vec<String> = skip_hosts.iter().map(|h| format!("x'{h}'")).collect();
            let filter = match (table.as_str(), skipped.is_empty()) {
                (_, true) => String::new(),
                ("chart", false) => format!(" WHERE host_id NOT IN ({})", skipped.join(", ")),
                ("dimension" | "chart_label", false) => format!(
                    " WHERE chart_id NOT IN (SELECT chart_id FROM chart WHERE host_id IN ({}))",
                    skipped.join(", ")
                ),
                _ => String::new(),
            };
            let mut stmt = c.prepare(&format!(
                "SELECT * FROM \"{}\"{filter} ORDER BY rowid",
                table.replace('"', "\"\"")
            ))?;
            let names: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
            let mut rows = stmt.query([])?;
            let mut lines = Vec::new();
            while let Some(r) = rows.next()? {
                let mut line = format!("row {table}");
                for (i, name) in names.iter().enumerate() {
                    let column = format!("{table}.{name}");
                    let masked = masks.iter().any(|m| m == name || *m == column);
                    let v = match r.get_ref(i)? {
                        _ if masked => "<masked>".into(),
                        ValueRef::Blob(b) if b.len() == 16 && aliased.contains(&column) => {
                            let next = aliases.len() + 1;
                            format!("u{}", aliases.entry(b.to_vec()).or_insert(next))
                        }
                        v => value(v),
                    };
                    line.push_str(&format!(" {name}={v}"));
                }
                lines.push(line);
            }
            if sorted.contains(table) {
                lines.sort();
            }
            for line in lines {
                println!("{line}");
            }
        }
        Ok(())
    };
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{path}: {err}");
            ExitCode::from(1)
        }
    }
}
