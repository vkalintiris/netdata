//! Run an ad-hoc SQL query over a single SFST file.
//!
//! ```text
//! cargo run -p sfst-datafusion --example sql -- <file.sfst> "<SQL>"
//! ```
//!
//! The file is registered as the table `logs`. Column names are the field
//! paths (quote dotted names): `SELECT timestamp, "body.commit.record.text"
//! FROM logs WHERE "attributes.bluesky.did" = '…' ORDER BY timestamp LIMIT 20`.

use std::sync::Arc;

use sfst_datafusion::SfstTable;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(sql)) = (args.next(), args.next()) else {
        eprintln!(r#"usage: sql <file.sfst> "<SQL>"   (the file is table `logs`)"#);
        std::process::exit(2);
    };

    let table = SfstTable::open_path(&path)?;
    // Pushdown-enabled context: COUNT(*) GROUP BY <field> hits the facet bitmaps.
    let ctx = sfst_datafusion::session_context();
    ctx.register_table("logs", Arc::new(table))?;

    ctx.sql(&sql).await?.show().await?;
    Ok(())
}
