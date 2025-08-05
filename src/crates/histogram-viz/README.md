# histogram-viz

A command-line tool for visualizing histograms from journal file indexes using interactive terminal charts powered by ratatui.

## Features

- **Interactive visualization** with full-screen charts
- **User-friendly timestamps** on X-axis (formatted as DD/MM HH:MM)
- **Labeled Y-axis** with count values
- **Color-coded charts** with borders and titles
- **Smooth line rendering** using Braille characters
- Build file indexes from journal files with configurable bucket sizes
- Filter and visualize histograms for specific field values
- Display comprehensive statistics including time ranges, bucket counts, and top entries

## Installation

```bash
cargo build --release
```

The binary will be available at `target/release/histogram-viz`.

## Usage

```bash
histogram-viz [OPTIONS] <FILE>

Arguments:
  <FILE>  Path to the journal file

Options:
  -f, --fields <FIELDS>            Field names to index (comma-separated)
  -b, --bucket-size <BUCKET_SIZE>  Bucket size in seconds [default: 1]
  -v, --values <VALUES>            Show histograms for specific field values (comma-separated)
  -h, --help                       Print help
  -V, --version                    Print version
```

## Examples

### Basic usage with default settings

```bash
histogram-viz -f "PRIORITY" /var/log/journal/.../system.journal
```

This will:
- Index the PRIORITY field with 1-second buckets
- Display statistics about the data
- Show an interactive full-screen histogram chart
- List available field values

### Visualize specific field values

```bash
histogram-viz \
  -f "PRIORITY" \
  -v "PRIORITY=6" \
  /var/log/journal/.../system.journal
```

This displays:
1. Overall histogram for all entries
2. Separate histogram for PRIORITY=6 entries

Each histogram shows:
- Statistics summary
- Interactive full-screen chart with labeled axes
- Press any key to move to the next chart or exit

### Customize bucket size

```bash
histogram-viz \
  -f "log.attributes.method,log.attributes.status" \
  -b 60 \
  -v "log.attributes.method=GET,log.attributes.status=200.0" \
  /path/to/file.journal
```

This uses 60-second buckets and shows histograms for GET requests and 200 status codes.

## Interactive Controls

When viewing a chart:
- **Press any key** to exit the current chart and continue
- Charts are displayed full-screen with proper axis labels
- X-axis shows timestamps in European format (DD/MM HH:MM)
- Y-axis shows count values

## Output

The tool provides:

1. **Statistics Summary**:
   - Time range with formatted timestamps
   - Duration of the data
   - Bucket size and count
   - Total entries
   - Max count per bucket
   - Top 10 buckets by count

2. **Interactive Chart**:
   - Full-screen visualization
   - Colored borders and title
   - X-axis with formatted timestamps
   - Y-axis with count labels
   - Smooth line chart using Braille characters

3. **Field-specific histograms**: When using `-v`, shows separate charts for each specified field value

## Dependencies

- **`ratatui`**: For terminal user interface and chart rendering
- **`crossterm`**: For terminal manipulation and cross-platform support
- **`journal`**: For reading and indexing journal files
- **`clap`**: For command-line argument parsing
- **`chrono`**: For timestamp formatting

## Technical Details

The tool uses:
- Ratatui's Chart widget with GraphType::Line for smooth visualization
- Braille marker characters for high-resolution terminal rendering
- Automatic axis scaling and labeling
- Color-coded UI elements for better readability
- Full terminal takeover for immersive chart viewing
