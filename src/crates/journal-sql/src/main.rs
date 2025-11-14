//! Journal SQL - Query systemd journal logs using SQL
//!
//! This binary provides a SQL interface to systemd journal logs using Apache DataFusion.

mod table_provider;
mod time_parser;

use anyhow::{Context, Result};
use clap::Parser;
use datafusion::prelude::*;
use journal_function::charts::FileIndexingMetrics;
use journal_function::indexing::IndexingService;
use journal_function::{FileIndexCache, Monitor, Registry};
use std::path::PathBuf;
use std::sync::Arc;
use table_provider::JournalTableProvider;
use time_parser::parse_time_spec;
use tracing::{Level, info};
use tracing_subscriber;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to journal directory (default: /var/log/journal)
    #[arg(short, long, default_value = "/var/log/journal")]
    journal_path: PathBuf,

    /// SQL query to execute (if not provided, enters interactive mode)
    #[arg(short, long)]
    query: Option<String>,

    /// Start showing entries on or newer than the specified date.
    /// Accepts: "now", "today", "yesterday", "-1h", "-2days", "2025-01-12", "2025-01-12 14:30:00"
    #[arg(short = 'S', long)]
    since: Option<String>,

    /// Stop showing entries on or older than the specified date.
    /// Accepts: "now", "today", "yesterday", "-1h", "-2days", "2025-01-12", "2025-01-12 14:30:00"
    #[arg(short = 'U', long)]
    until: Option<String>,

    /// Facet fields to index (comma-separated)
    #[arg(long, default_value = "PRIORITY,_SYSTEMD_UNIT,SYSLOG_IDENTIFIER")]
    facets: String,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing
    let level = if args.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };
    tracing_subscriber::fmt().with_max_level(level).init();

    info!("Starting journal-sql");
    info!("Journal path: {:?}", args.journal_path);

    // Parse facets
    let facets: Vec<String> = args
        .facets
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    info!("Facets: {:?}", facets);

    // Parse time range
    let after = if let Some(since_str) = &args.since {
        parse_time_spec(since_str).context("Failed to parse --since time")?
    } else {
        0 // Beginning of time
    };

    let before = if let Some(until_str) = &args.until {
        parse_time_spec(until_str).context("Failed to parse --until time")?
    } else {
        // Default to now
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32
    };

    info!("Time range: {} to {} (unix timestamps)", after, before);

    // Set up journal registry and file index cache
    let (monitor, _event_rx) = Monitor::new().context("Failed to create monitor")?;
    let registry = Registry::new(monitor);

    // Watch the journal directory
    registry
        .watch_directory(args.journal_path.to_str().context("Invalid journal path")?)
        .context("Failed to watch journal directory")?;

    info!("Watching journal directory: {:?}", args.journal_path);

    // Give the registry a moment to scan the directory
    // tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Create file index cache (using simple HashMap for now)
    let file_index_cache = FileIndexCache::with_hashmap(std::collections::HashMap::new());

    // Create a metrics handle (we're not reporting metrics to Netdata, but IndexingService needs it)
    // We create a minimal PluginRuntime just to get a ChartHandle
    use rt::PluginRuntime;
    let mut temp_runtime = PluginRuntime::new("temp");
    let metrics = temp_runtime.register_chart(
        FileIndexingMetrics::default(),
        std::time::Duration::from_secs(1),
    );
    // We don't actually run the runtime, just use it to create the handle
    drop(temp_runtime);

    // Create indexing service
    let indexing_service = IndexingService::new(
        file_index_cache.clone(),
        registry.clone(),
        4,   // 4 worker threads
        100, // queue capacity
        metrics,
    );

    info!("Created indexing service with 4 workers");

    // Create DataFusion session context
    let ctx = SessionContext::new();

    // Create and register the journal table provider
    let table_provider = Arc::new(JournalTableProvider::new(
        registry,
        file_index_cache,
        indexing_service,
        after,
        before,
        facets,
    ));

    ctx.register_table("journal", table_provider)
        .context("Failed to register journal table")?;

    info!("Registered 'journal' table");

    // Execute query or enter interactive mode
    if let Some(query) = args.query {
        execute_query(&ctx, &query).await?;
    } else {
        interactive_mode(&ctx).await?;
    }

    Ok(())
}

async fn execute_query(ctx: &SessionContext, query: &str) -> Result<()> {
    info!("Executing query: {}", query);

    let df = ctx
        .sql(query)
        .await
        .context("Failed to create DataFrame from query")?;

    df.show().await.context("Failed to show query results")?;

    Ok(())
}

async fn interactive_mode(ctx: &SessionContext) -> Result<()> {
    use std::io::{self, Write};

    println!("Journal SQL Interactive Mode");
    println!("Enter SQL queries to query journal logs. Type 'exit' or 'quit' to exit.");
    println!("Example: SELECT timestamp, message FROM journal WHERE priority <= 3 LIMIT 10");
    println!();

    loop {
        print!("journal-sql> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        let query = input.trim();

        if query.is_empty() {
            continue;
        }

        if query.eq_ignore_ascii_case("exit") || query.eq_ignore_ascii_case("quit") {
            println!("Goodbye!");
            break;
        }

        if let Err(e) = execute_query(ctx, query).await {
            eprintln!("Error executing query: {}", e);
        }
    }

    Ok(())
}
