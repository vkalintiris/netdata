# Journal FST Indexer

A command-line tool for building FST (Finite State Transducer) indexes of data objects in systemd journal files.

## Overview

This tool reads a journal file and creates an FST-based index of all data objects. FST is a space-efficient data structure that enables fast prefix searches and lookups.

## Features

- **Two index types:**
  - `set`: Index just the keys (data object payloads)
  - `map`: Index keys with their hash values for lookup

- **Field filtering:** Index only specific fields (e.g., MESSAGE, PRIORITY)
- **Memory efficient:** FST provides compact representation
- **Fast lookups:** Efficient prefix search and exact match capabilities

## Usage

```bash
# Build a map index of all data objects
journal-fst-indexer --journal-file /path/to/journal.journal --output index.fst

# Build a set index (just keys, no offsets)
journal-fst-indexer -j /path/to/journal.journal -o index.fst --index-type set

# Index only specific fields
journal-fst-indexer -j journal.journal -o index.fst --field-filter "MESSAGE,PRIORITY"

# Limit indexing to first 1000 objects (for testing)
journal-fst-indexer -j journal.journal -o index.fst --limit 1000

# Enable verbose logging
journal-fst-indexer -j journal.journal -o index.fst --verbose
```

## Command-line Options

- `-j, --journal-file <PATH>`: Path to the journal file to index (required)
- `-o, --output <PATH>`: Path to output the FST index (required)
- `-t, --index-type <TYPE>`: Index type: "set" or "map" (default: "map")
- `-f, --field-filter <FIELDS>`: Comma-separated list of field names to index
- `-l, --limit <N>`: Maximum number of data objects to index
- `-v, --verbose`: Enable verbose logging

## Building

```bash
cd journal-fst-indexer
cargo build --release
```

## Example

```bash
# Index a journal file
./target/release/journal-fst-indexer \
  --journal-file /var/log/journal/system.journal \
  --output system.fst \
  --index-type map

# Output:
# INFO Journal FST Indexer
# INFO Journal file: "/var/log/journal/system.journal"
# INFO Output file: "system.fst"
# INFO Index type: Map
# INFO Opening journal file...
# INFO Journal file header:
# INFO   Entries: 12345
# INFO   Objects: 45678
# INFO Collected 45678 data objects total
# INFO Building FST Map index
# INFO Building index from 38291 unique keys
# INFO FST Map index written to "system.fst"
# INFO Index statistics:
# INFO   Type: Map
# INFO   Size: 2451234 bytes (2.34 MB)
# INFO   Keys: 38291
# INFO Indexing complete!
```

## Use Cases

1. **Fast field value lookups**: Quickly check if a specific field value exists in the journal
2. **Prefix searches**: Find all data objects with payloads starting with a prefix
3. **Deduplication**: Identify unique field values in large journal files
4. **Query optimization**: Pre-build indexes for faster journal queries

## Notes

- FST requires keys to be sorted, which is handled automatically
- Duplicate keys are deduplicated (first occurrence is kept for map indexes)
- The hash value stored in map indexes corresponds to the journal's internal hash
- FST indexes are read-only after creation (use mmap for efficient loading)
