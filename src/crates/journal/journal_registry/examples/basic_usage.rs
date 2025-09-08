use journal_registry::{JournalRegistry, JournalSourceType, SortBy, SortOrder};
use std::time::{Duration, SystemTime};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    let registry = JournalRegistry::new()?;
    info!("Journal registry initialized");

    for dir in ["/var/log/journal", "/run/log/journal"] {
        match registry.add_directory(dir) {
            Ok(_) => info!("Added directory: {}", dir),
            Err(e) => warn!("Failed to add directory {}: {}", dir, e),
        }
    }

    // Display initial statistics
    println!("\n=== Journal Files Statistics ===");
    println!("Total files: {}", registry.query().count());
    println!(
        "Total size: {:.2} MB",
        registry.query().total_size() as f64 / (1024.0 * 1024.0)
    );

    // Get system journal files sorted by size
    println!("\n=== System Journal Files (sorted by size) ===");
    let system_files = registry
        .query()
        .source(JournalSourceType::System)
        .sort_by(SortBy::Size(SortOrder::Descending))
        .execute();

    println!("Found {} system journal files:", system_files.len());
    for (idx, file) in system_files.iter().take(5).enumerate() {
        println!(
            "  [{}] {} ({:.2} MB) - modified: {:?}",
            idx,
            file.path.display(),
            file.size as f64 / (1024.0 * 1024.0),
            file.modified
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|_| {
                    format!(
                        "{} hours ago",
                        (SystemTime::now()
                            .duration_since(file.modified)
                            .unwrap_or_default()
                            .as_secs()
                            / 3600)
                    )
                })
                .unwrap_or_else(|_| "unknown".to_string())
        );
    }

    // Recent large files (modified in last 24 hours, > 1MB)
    println!("\n=== Recent Large Files (last 24h, >1MB) ===");
    let recent_large = registry
        .query()
        .modified_after(SystemTime::now() - Duration::from_secs(24 * 60 * 60))
        .min_size(1024 * 1024) // 1MB
        .sort_by(SortBy::Modified(SortOrder::Descending))
        .limit(10)
        .execute();

    if recent_large.is_empty() {
        println!("No large files modified in the last 24 hours");
    } else {
        println!(
            "Found {} large files modified recently:",
            recent_large.len()
        );
        for file in &recent_large {
            println!(
                "  {} ({:.2} MB) - {}",
                file.path.file_name().unwrap_or_default().to_string_lossy(),
                file.size as f64 / (1024.0 * 1024.0),
                file.source_type
            );
        }
    }

    // Group files by source type
    println!("\n=== Files by Source Type ===");
    for source_type in &[
        JournalSourceType::System,
        JournalSourceType::User,
        JournalSourceType::Remote,
        JournalSourceType::Namespace,
        JournalSourceType::Other,
    ] {
        let files = registry.query().source(*source_type).execute();
        let total_size = registry.query().source(*source_type).total_size();

        if !files.is_empty() {
            println!(
                "  {:10} - {} files, {:.2} MB total",
                source_type.to_string(),
                files.len(),
                total_size as f64 / (1024.0 * 1024.0)
            );
        }
    }

    // Find files by machine ID (if any exist)
    println!("\n=== Files by Machine ID ===");
    let all_files = registry.query().execute();
    let machine_ids: std::collections::HashSet<_> = all_files
        .iter()
        .filter_map(|f| f.machine_id.as_ref())
        .cloned()
        .collect();

    if machine_ids.is_empty() {
        println!("No files with machine IDs found");
    } else {
        for (idx, machine_id) in machine_ids.iter().take(3).enumerate() {
            let machine_files = registry
                .query()
                .machine(machine_id)
                .sort_by(SortBy::Sequence(SortOrder::Ascending))
                .execute();

            println!(
                "  Machine {} ({}...): {} files",
                idx + 1,
                &machine_id[..8.min(machine_id.len())],
                machine_files.len()
            );
        }
    }

    Ok(())
}