use journal::file::Mmap;
use journal::{Direction, JournalFile, JournalReader, Location};
use std::env;
use std::process;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get journal file path from command line arguments
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <journal-file-path>", args[0]);
        process::exit(1);
    }
    let journal_path = &args[1];

    // Open the journal file
    let window_size = 8 * 1024 * 1024; // 8 MiB window
    let journal_file = JournalFile::<Mmap>::open(journal_path, window_size)?;

    // Create a reader
    let mut reader = JournalReader::default();

    // Seek to head and get seqnum
    println!("Seeking to HEAD...");
    reader.set_location(Location::Head);
    if reader.step(&journal_file, Direction::Forward)? {
        let (seqnum, seqnum_id) = reader.get_seqnum(&journal_file)?;
        println!("  HEAD seqnum: {}", seqnum);
        println!("  HEAD seqnum_id: {}", hex::encode(seqnum_id));
    } else {
        println!("  No entries found (empty journal)");
    }

    // Seek to tail and get seqnum
    println!("\nSeeking to TAIL...");
    reader.set_location(Location::Tail);
    if reader.step(&journal_file, Direction::Backward)? {
        let (seqnum, seqnum_id) = reader.get_seqnum(&journal_file)?;
        println!("  TAIL seqnum: {}", seqnum);
        println!("  TAIL seqnum_id: {}", hex::encode(seqnum_id));
    } else {
        println!("  No entries found (empty journal)");
    }

    Ok(())
}
