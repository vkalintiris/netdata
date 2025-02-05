use error::NdError;
use gorilla::{GorillaBuffer, GorillaReader, GorillaWriter, SeriesIdSlice};
use rand::Rng;
use std::num::NonZeroU32;
use tqdm::Style;
use tracing::{info, Level};
use tracing_subscriber::fmt;

const NUM_SERIES: usize = 5;
const PAYLOAD_BYTES: usize = 1024 * 1024;
const NUM_ITERATIONS: usize = 1024 * 1024;

fn init_logging() {
    fmt()
        .with_ansi(false)
        .with_thread_ids(true)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_level(true)
        .with_max_level(Level::ERROR)
        .init();
}

fn run_iteration(iteration: usize) -> Result<(), NdError> {
    let mut rng = rand::rng();

    // Generate random number of series (1-2)
    let num_series = rng.random_range(1..=NUM_SERIES);

    // Generate random series IDs (1 to NUM_SERIES)
    let mut series_ids = Vec::with_capacity(num_series);
    let mut used_ids = std::collections::HashSet::new();

    while series_ids.len() < num_series {
        let id = rng.random_range(1..=NUM_SERIES as u32);
        if used_ids.insert(id) {
            series_ids.push(id);
        }
    }
    series_ids.sort_unstable();

    // Generate random initial values
    let initial_values: Vec<u32> = (0..num_series).map(|_| rng.random_range(0..1024)).collect();

    info!(
        iteration = iteration,
        num_series = num_series,
        series_ids = ?series_ids,
        initial_values = ?initial_values,
        "Starting new iteration"
    );

    // Create a new buffer
    let timestamp = NonZeroU32::new(1000).unwrap();
    let series_slice = SeriesIdSlice::new(&series_ids).unwrap();
    let mut buffer =
        GorillaBuffer::<NUM_SERIES, PAYLOAD_BYTES>::new(timestamp, &series_slice, &initial_values)?;

    // Create writer and start adding random values
    let mut writer = GorillaWriter::new(&mut buffer);
    let mut all_written_values = Vec::new();

    let mut iterations = 0;
    loop {
        let new_values: Vec<u32> = (0..num_series).map(|_| rng.random_range(0..1024)).collect();

        match writer.add_samples(&new_values) {
            Ok(()) => {
                all_written_values.push(new_values);
            }
            Err(NdError::NoSpace) => {
                info!(
                    samples_written = all_written_values.len(),
                    "Buffer full, stopping writes"
                );
                break;
            }
            Err(e) => return Err(e),
        }

        iterations += 1;
        if iterations == u16::MAX {
            break;
        }
    }

    // Create reader and verify values
    let mut reader = GorillaReader::new(&buffer);

    // Then verify all subsequent values
    for (i, expected_values) in all_written_values.iter().enumerate() {
        let read_values = reader.read_samples().unwrap();
        assert_eq!(
            read_values,
            expected_values.as_slice(),
            "Values mismatch at sample {}",
            i
        );
    }

    // Ensure we've read everything
    assert!(
        reader.read_samples().is_none(),
        "Expected no more samples but got some"
    );

    info!(
        iteration = iteration,
        total_samples = all_written_values.len() + 1,
        "Iteration completed successfully"
    );

    Ok(())
}

fn main() {
    init_logging();

    for i in tqdm::tqdm(0..NUM_ITERATIONS)
        .style(Style::Balloon)
        .desc(Some("some description"))
    {
        if let Err(e) = run_iteration(i) {
            eprintln!("Error in iteration {}: {:?}", i, e);
            break;
        }
    }
}
