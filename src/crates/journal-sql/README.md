# journal-sql

Query systemd journal logs using SQL with Apache DataFusion.

## Overview

`journal-sql` is a binary crate that provides a SQL interface to systemd journal logs. It uses Apache DataFusion to enable powerful SQL queries over journal log data, making it easy to analyze and explore system logs.

## Features

- **SQL Querying**: Use standard SQL to query journal logs
- **User-Friendly Time Filtering**: Specify time ranges with natural expressions like "today", "yesterday", "-1h", "-2days", or absolute dates
- **Faceted Indexing**: Index logs by custom fields for efficient querying
- **Interactive Mode**: Enter SQL queries interactively
- **DataFusion Integration**: Leverages Apache DataFusion's query optimizer and execution engine

## Building

From the workspace root:

```bash
cargo build -p journal-sql
```

Or from the journal-sql directory:

```bash
cargo build
```

## Usage

### Basic Usage

Query journal logs:

```bash
# Query all logs
./journal-sql --query "SELECT timestamp, priority, message FROM journal WHERE priority <= 3 LIMIT 10"

# Query logs from today
./journal-sql --since "today" --query "SELECT COUNT(*) FROM journal"

# Query logs from the last hour
./journal-sql --since="-1h" --query "SELECT priority, COUNT(*) FROM journal GROUP BY priority"

# Query logs from yesterday
./journal-sql --since "yesterday" --until "today" --query "SELECT * FROM journal LIMIT 100"

# Query logs from a specific date
./journal-sql --since "2025-11-13" --query "SELECT * FROM journal LIMIT 10"

# Query logs from a specific datetime
./journal-sql --since "2025-11-13 14:30:00" --query "SELECT * FROM journal"
```

### Interactive Mode

Enter interactive mode to run multiple queries:

```bash
./journal-sql
```

Then enter SQL queries at the prompt:

```sql
journal-sql> SELECT timestamp, syslog_identifier, message FROM journal WHERE priority = 0 LIMIT 5
journal-sql> SELECT COUNT(*) as error_count FROM journal WHERE priority <= 3
journal-sql> SELECT systemd_unit, COUNT(*) as count FROM journal GROUP BY systemd_unit ORDER BY count DESC LIMIT 10
journal-sql> exit
```

### Command-Line Options

- `-j, --journal-path <PATH>` - Path to journal directory (default: `/var/log/journal`)
- `-q, --query <SQL>` - SQL query to execute (if not provided, enters interactive mode)
- `-S, --since <TIME>` - Start showing entries on or newer than the specified date (default: beginning of time)
- `-U, --until <TIME>` - Stop showing entries on or older than the specified date (default: now)
- `--facets <FIELDS>` - Comma-separated list of fields to index (default: `PRIORITY,_SYSTEMD_UNIT,SYSLOG_IDENTIFIER`)
- `-v, --verbose` - Enable verbose logging

### Time Format Options

The `--since` and `--until` options accept various time formats (all times are interpreted in local timezone):

**Special Keywords:**
- `now` - Current time
- `today` - Start of today (00:00:00)
- `yesterday` - Start of yesterday (00:00:00)

**Relative Time (going back from now):**
- `-1h`, `-2hours` - Hours ago
- `-30m`, `-45minutes` - Minutes ago
- `-1d`, `-7days` - Days ago
- `-1w`, `-2weeks` - Weeks ago
- `-30s`, `-60seconds` - Seconds ago

**Absolute Dates and Times:**
- `2025-11-13` - Specific date (start of day)
- `2025-11-13 14:30:00` - Specific date and time
- `2025/11/13` - Alternative date format
- `2025-11-13T14:30:00` - ISO 8601 format

**Examples:**
```bash
# Last hour
--since="-1h"

# Yesterday only
--since "yesterday" --until "today"

# Specific date range
--since "2025-11-01" --until "2025-11-13"

# Last 7 days
--since="-7days"

# From specific time to now
--since "2025-11-13 09:00:00"
```

## Schema

The journal table exposes the following columns:

| Column              | Type                  | Description                           |
|---------------------|-----------------------|---------------------------------------|
| `timestamp`         | Timestamp(Microsecond)| Entry timestamp in microseconds       |
| `priority`          | UInt32 (nullable)     | Syslog priority level (0-7)           |
| `message`           | String (nullable)     | Log message text                      |
| `syslog_identifier` | String (nullable)     | Syslog identifier                     |
| `systemd_unit`      | String (nullable)     | Systemd unit name                     |
| `hostname`          | String (nullable)     | Hostname                              |
| `pid`               | String (nullable)     | Process ID                            |
| `uid`               | String (nullable)     | User ID                               |
| `gid`               | String (nullable)     | Group ID                              |
| `comm`              | String (nullable)     | Command name                          |
| `exe`               | String (nullable)     | Executable path                       |
| `cmdline`           | String (nullable)     | Command line                          |
| `boot_id`           | String (nullable)     | Boot ID                               |
| `machine_id`        | String (nullable)     | Machine ID                            |

## Example Queries

### Find all error messages

```sql
SELECT timestamp, syslog_identifier, message
FROM journal
WHERE priority <= 3
ORDER BY timestamp DESC
LIMIT 20
```

### Count messages by systemd unit

```sql
SELECT systemd_unit, COUNT(*) as count
FROM journal
GROUP BY systemd_unit
ORDER BY count DESC
LIMIT 10
```

### Find messages from a specific service

```sql
SELECT timestamp, message
FROM journal
WHERE systemd_unit = 'sshd.service'
ORDER BY timestamp DESC
```

### Analyze error patterns

```sql
SELECT
    syslog_identifier,
    COUNT(*) as error_count,
    MIN(timestamp) as first_error,
    MAX(timestamp) as last_error
FROM journal
WHERE priority <= 3
GROUP BY syslog_identifier
ORDER BY error_count DESC
```

## Architecture

### Components

1. **JournalTableProvider**: Implements DataFusion's `TableProvider` trait to expose journal logs as a queryable table
2. **Registry**: Tracks available journal files and their metadata
3. **FileIndexCache**: Caches indexed journal files for efficient querying
4. **LogQuery**: Executes queries over indexed journal files

### Data Flow

1. The tool watches the journal directory and discovers available log files
2. Files are indexed on-demand based on the requested facets
3. SQL queries are parsed by DataFusion
4. The JournalTableProvider converts journal entries to Apache Arrow RecordBatches
5. DataFusion's execution engine processes the query and returns results

## Performance Considerations

- **Indexing**: The first query may take longer as files are indexed
- **Facets**: Choose facets that match your query patterns for best performance
- **Time Ranges**: Narrow time ranges improve query performance
- **Caching**: Indexed files are cached in memory using a HashMap (configurable to use disk-backed Foyer cache)

## Limitations

- Currently uses a simple HashMap cache (limited to available memory)
- Default limit of 100,000 log entries per query (configurable in code)
- Schema is fixed at startup (no dynamic column discovery)
- Requires read access to journal files (typically requires root or membership in systemd-journal group)

## Future Enhancements

- [ ] Configurable query limits
- [ ] Disk-backed cache using Foyer hybrid cache
- [ ] Dynamic schema discovery
- [ ] Push-down predicates for more efficient filtering
- [ ] Projection push-down to read only requested fields
- [ ] Support for more journal fields
- [ ] Export query results to various formats (CSV, JSON, Parquet)
- [ ] Real-time log streaming

## Dependencies

- **datafusion**: SQL query engine
- **journal**: Low-level journal file parser
- **journal-function**: Journal indexing and querying functionality
- **arrow**: Columnar data format
- **tokio**: Async runtime
- **clap**: Command-line argument parsing

## See Also

- [Apache DataFusion](https://datafusion.apache.org/)
- [Apache Arrow](https://arrow.apache.org/)
- [systemd journal documentation](https://www.freedesktop.org/software/systemd/man/systemd-journald.service.html)
