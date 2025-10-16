use clap::Parser;
use journal::file::JournalFileMap;
use std::io;

mod ratatui_viz;

#[derive(Parser, Debug)]
#[command(
    name = "histogram-viz",
    about = "Visualize histograms from journal file indexes using interactive charts",
    version
)]
struct Args {
    /// Path to the journal file
    #[arg(value_name = "FILE")]
    file: String,
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    // Open the journal file
    let window_size = 8 * 1024 * 1024;
    let journal_file = match JournalFileMap::open(&args.file, window_size) {
        Ok(jf) => jf,
        Err(e) => {
            eprintln!("Failed to open {}: {}", args.file, e);
            return Err(io::Error::other(e));
        }
    };

    ratatui_viz::visualize_histogram_interactive(&journal_file, "Overall Histogram".to_string())?;

    Ok(())
}
