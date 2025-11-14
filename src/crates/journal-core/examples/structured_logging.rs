#![cfg(feature = "serde-api")]

use journal::log::{Config, Log, RetentionPolicy, RotationPolicy};
use journal::repository::{Origin, Source as JournalSource};
use serde::Serialize;

#[derive(Serialize)]
struct LogEntry {
    message: String,
    level: String,
    source: Source,
    metrics: Metrics,
}

#[derive(Serialize)]
struct Source {
    file: String,
    line: u32,
    function: String,
}

#[derive(Serialize)]
struct Metrics {
    response_time_ms: u64,
    status_code: u16,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let journal_path = std::path::Path::new("/home/vk/repos/tmp/structured-logs");
    let origin = Origin {
        machine_id: None,
        namespace: None,
        source: JournalSource::User(1000), // UID 1000
    };
    let config = Config::new(
        origin,
        RotationPolicy::default(),
        RetentionPolicy::default(),
    );
    let mut log = Log::new(journal_path, config)?;

    // Example 1: Simple structured log entry
    let entry = LogEntry {
        message: "User authentication successful".to_string(),
        level: "INFO".to_string(),
        source: Source {
            file: "auth.rs".to_string(),
            line: 142,
            function: "authenticate_user".to_string(),
        },
        metrics: Metrics {
            response_time_ms: 45,
            status_code: 200,
        },
    };

    println!("Writing structured log entry...");
    log.write_structured(&entry)?;
    log.sync()?;

    println!("✓ Entry written successfully!");
    println!("\nThe following fields were written to the journal:");
    println!("  MESSAGE=User authentication successful");
    println!("  LEVEL=INFO");
    println!("  SOURCE_FILE=auth.rs");
    println!("  SOURCE_LINE=142");
    println!("  SOURCE_FUNCTION=authenticate_user");
    println!("  METRICS_RESPONSE_TIME_MS=45");
    println!("  METRICS_STATUS_CODE=200");

    // Example 2: Multiple entries
    println!("\n\nWriting multiple entries...");
    for i in 0..5 {
        let entry = LogEntry {
            message: format!("Request {} processed", i),
            level: "DEBUG".to_string(),
            source: Source {
                file: "handler.rs".to_string(),
                line: 89 + i,
                function: "handle_request".to_string(),
            },
            metrics: Metrics {
                response_time_ms: 10 + i as u64,
                status_code: 200,
            },
        };
        log.write_structured(&entry)?;
    }
    log.sync()?;

    println!("✓ {} entries written successfully!", 5);

    Ok(())
}
