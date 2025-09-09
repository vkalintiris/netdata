use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use datafusion::prelude::*;
use tracing::{info, warn};

// mod journal_provider;  // Commenting out for now due to API compatibility issues
mod simple_journal_provider;

// use journal_provider::JournalTableProvider;
use simple_journal_provider::SimpleJournalProvider;

#[derive(Parser)]
#[command(name = "journal-datafusion")]
#[command(about = "A DataFusion-powered SQL interface to systemd journal files")]
struct Args {
    /// Directory paths containing journal files (can be specified multiple times)
    #[arg(short, long, value_name = "DIRECTORY")]
    journal_dirs: Vec<String>,

    /// SQL query to execute
    #[arg(short, long, value_name = "SQL")]
    query: Option<String>,

    /// Interactive mode (starts a REPL)
    #[arg(short, long)]
    interactive: bool,

    /// Verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing
    let level = if args.verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };
    
    tracing_subscriber::fmt()
        .with_max_level(level)
        .init();

    info!("Starting journal-datafusion");

    // Default to common journal directories if none specified
    let journal_dirs = if args.journal_dirs.is_empty() {
        vec![
            "/var/log/journal".to_string(),
            "/run/log/journal".to_string(),
        ]
    } else {
        args.journal_dirs
    };

    // Create DataFusion session context
    let ctx = SessionContext::new();

    // TODO: Register journal metadata table provider (commented out due to API issues)
    // let journal_files_provider = Arc::new(JournalTableProvider::new(journal_dirs.clone()).await?);
    // ctx.register_table("journal_files", journal_files_provider)?;

    // Register simplified journal entries table provider  
    let journal_entries_provider = Arc::new(SimpleJournalProvider::new(journal_dirs).await?);
    ctx.register_table("journal", journal_entries_provider)?;

    info!("Journal tables registered with DataFusion");

    if let Some(query) = args.query {
        // Execute single query
        execute_query(&ctx, &query).await?;
    } else if args.interactive {
        // Start interactive mode
        start_interactive_mode(&ctx).await?;
    } else {
        // Default: show available tables and basic info
        show_info(&ctx).await?;
    }

    Ok(())
}

async fn execute_query(ctx: &SessionContext, query: &str) -> Result<()> {
    info!("Executing query: {}", query);
    
    let df = ctx.sql(query).await?;
    let results = df.collect().await?;
    
    for batch in results {
        println!("{}", arrow::util::pretty::pretty_format_batches(&[batch])?);
    }
    
    Ok(())
}

async fn start_interactive_mode(ctx: &SessionContext) -> Result<()> {
    println!("Journal DataFusion Interactive Mode");
    println!("Type your SQL queries below. Type 'exit' or 'quit' to exit.");
    println!("Available tables:");
    println!("  - 'journal' - journal entries (simplified demo version)");
    println!();

    loop {
        print!("journal-sql> ");
        std::io::Write::flush(&mut std::io::stdout())?;
        
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        
        if matches!(input.to_lowercase().as_str(), "exit" | "quit") {
            println!("Goodbye!");
            break;
        }
        
        if let Err(e) = execute_query(ctx, input).await {
            eprintln!("Error executing query: {}", e);
        }
    }
    
    Ok(())
}

async fn show_info(ctx: &SessionContext) -> Result<()> {
    println!("Journal DataFusion SQL Interface");
    println!("================================");
    println!();
    
    // Show table info
    println!("Available tables:");
    println!("  - 'journal' - journal entries (simplified demo version)");
    
    // Show sample queries
    println!();
    println!("Sample queries:");
    println!("  SELECT COUNT(*) FROM journal;");
    println!("  SELECT source_file, file_size FROM journal LIMIT 10;");
    println!("  SELECT * FROM journal WHERE file_size > 1000000;");
    println!();
    println!("Use --query \"<SQL>\" to execute a specific query");
    println!("Use --interactive to start interactive mode");
    
    Ok(())
}