# Libnetdata Facets Implementation - Key Findings Summary

## Quick Reference

### A. WHAT FACETED DATA EMITS

The facets system emits **five major components** in its JSON output:

1. **Facets Array** - Available filter options with value counts
   ```json
   {
     "id": "value_id",
     "name": "displayed_name",    // May be transformed
     "count": 42,                  // Rows with this value
     "order": 0                    // Display order
   }
   ```

2. **Columns Definition** - Field schema with metadata
   ```json
   {
     "id": "field_id",
     "name": "field_name",
     "type": "STRING",
     "visual": "VALUE|RICH",
     "transform": "NONE|XML",
     "filter": "FACET|NONE",
     "options": "VISIBLE|HIDDEN|STICKY|..."
   }
   ```

3. **Data Rows** - Actual log entries
   ```json
   [
     timestamp_usec,
     { "severity": "debug|normal|notice|warning|critical" },
     value1, value2, ..., valueN  // One per column
   ]
   ```

4. **Histogram** - Time-based distribution by facet values
   - X-axis: Time slots (microseconds)
   - Y-axis: Count per value per time slot
   - Shows: min, max, avg, percentage per dimension

5. **Statistics** - Metadata about the query
   - Rows evaluated/matched/returned/skipped
   - Sampling and estimation indicators
   - Processing performance metrics

---

## B. VALUE TRANSFORMATION AND REMAPPING SYSTEM

### 1. Transformation Points

Values can be transformed at **5 different points**:

| Scope | When | Where | Effect | Use Case |
|-------|------|-------|--------|----------|
| `FACETS_TRANSFORM_VALUE` | Ingestion | `facets_key_check_value()` | Affects indexing | Canonical form for filtering |
| `FACETS_TRANSFORM_FACET` | Output | `facets_report()` | Display in filters | User-friendly filter labels |
| `FACETS_TRANSFORM_HISTOGRAM` | Output | `facets_report()` | Display in legend | Time-series dimension names |
| `FACETS_TRANSFORM_DATA` | Output | `facets_report()` | Display in rows | Table cell rendering |
| `FACETS_TRANSFORM_FACET_SORT` | Output | `facets_sort_and_reorder_values()` | Affects sorting | Custom sort order |

### 2. Two Transformation Modes

#### Normal Transformation (view_only=false)
- Applied during **data ingestion**
- Transformed value is what gets **indexed and counted**
- Facet values reflect the transformed form
- Filtering matches transformed values
- **Use**: Normalize/canonicalize values before processing

#### View-Only Transformation (FACET_KEY_OPTION_TRANSFORM_VIEW)
- Applied only during **output generation**
- Raw values are **indexed as-is**
- Transformed display doesn't affect filtering
- Transparent to faceting logic
- **Use**: Format for display without changing semantic meaning

### 3. Transformation Pipeline Example

```
Input: "ERROR"
  |
  +---> [Normal Transform registered]
        If contains "ERROR" -> transform to "ERR"
        Index as "ERR"
        Facet value = "ERR"
        |
        v
  +---> Filtering works on "ERR"
        User sees "ERR" in facets
        |
        v
  +---> Output rows with "ERR"

---OR---

Input: "ERROR"
  |
  +---> [View-only Transform registered]
        Index as "ERROR" (raw)
        |
        v
  +---> Output phase - transform to "🔴 ERROR" for display
        Filtering still works on "ERROR"
        User sees "🔴 ERROR" in UI
        But facet counts based on "ERROR"
```

### 4. Value Access Pattern

When outputting a value:

```c
const char *value = facets_key_get_value(key);
// Returns:
//  - Buffer if FACET_KEY_VALUE_COPIED flag set (transformed)
//  - Raw pointer otherwise (untransformed)
```

**Key insight**: The COPIED flag tells us if transformation happened.

---

## C. SPECIAL VALUE MARKERS

Three hardcoded special values handle edge cases:

### Empty Values: "-"
- **When set**: Row finished but key had no value
- **Semantic**: Field was not present in this row
- **Flag**: `empty=true`
- **Output**: `null` in data array
- **In facets**: Shown as "-" option
- **Count**: Includes rows where field was missing

### Unsampled Values: "[unsampled]"
- **When set**: Called `facets_row_finished_unsampled()`
- **Semantic**: Data was present but skipped by sampling algorithm
- **Flag**: `unsampled=true`
- **Color**: "offline" (for UI)
- **In histogram**: Separate dimension showing skipped entries
- **Use**: Track data quality when sampling

### Estimated Values: "[estimated]"
- **When set**: Called `facets_update_estimations()`
- **Semantic**: Data was statistically extrapolated
- **Flag**: `estimated=true`
- **Color**: "generic" (for UI)
- **In histogram**: Separate dimension showing estimates
- **Use**: Represent data above sampling threshold

**Important**: These special values are never transformed. They use their hardcoded strings.

---

## D. COUNTING SEMANTICS

Two different counters track different things:

### `rows_matching_facet_value`
```
Incremented: For EVERY row containing this value
Meaning: "Total occurrences of this value in dataset"
Usage: Raw count before filtering
```

### `final_facet_value_counter`
```
Incremented: Only if ALL facet filters matched
Meaning: "Rows with this value AND all other filters matched"
Usage: Shows in facets array (reflects applied filters)
```

**Example**:
```
Dataset: 100 rows
Field "status" values: 80x"OK", 20x"ERROR"

rows_matching_facet_value["OK"] = 80     (Always 80)
final_facet_value_counter["OK"] = 50     (Only 50 matched other filters)

When displaying facets:
- Show count: 50 (final counter)
- User sees only the relevant options
```

---

## E. FIELD OPTIONS AND BEHAVIOR

```c
FACET_KEY_OPTION_FACET           // Enable as filter
FACET_KEY_OPTION_NO_FACET        // Don't index values
FACET_KEY_OPTION_NEVER_FACET     // Never allow as facet
FACET_KEY_OPTION_STICKY          // Always show in table
FACET_KEY_OPTION_VISIBLE         // Show by default
FACET_KEY_OPTION_FTS             // Include in full-text search
FACET_KEY_OPTION_MAIN_TEXT       // Full-width text field
FACET_KEY_OPTION_RICH_TEXT       // Support rich formatting
FACET_KEY_OPTION_HIDDEN          // Don't include in response
FACET_KEY_OPTION_FILTER_ONLY     // Filterable but not exposed
FACET_KEY_OPTION_TRANSFORM_VIEW  // View-only transformation
```

---

## F. OUTPUT FORMAT DECISIONS

### When Values Are Included/Excluded

**Facets Array**:
- Include: Non-empty, non-unsampled, non-estimated values
- Exclude: Empty values (if FACETS_OPTION_DONT_SEND_EMPTY_VALUE_FACETS set)
- Transform with: `FACETS_TRANSFORM_FACET` scope

**Data Rows**:
- Include: All values from stored rows
- Empty fields: Output as `null`
- Transform with: `FACETS_TRANSFORM_DATA` scope (if view-only)

**Histogram**:
- Include: Normal, unsampled, and estimated values as separate dimensions
- Transform with: `FACETS_TRANSFORM_HISTOGRAM` scope

---

## G. BUFFER MANAGEMENT

### Lifecycle
```
k->current_value.raw (pointer)
  |
  +---> [If transform/FTS needed]
        buffer_contents_replace(k->current_value.b, raw, len)
        Set FACET_KEY_VALUE_COPIED flag
  |
  +---> k->current_value.b (buffer)
        |
        +---> Passed to transform callback
              Modified by callback
        |
        +---> facets_key_get_value() retrieves it
              |
              v
        +---> Hashed for indexing
              |
              v
        +---> Stored in row's FACET_ROW_KEY_VALUE
              |
              v
        +---> Output in reports
```

### Key Points
- **Lazy allocation**: Buffer only created if needed
- **Copy-on-write style**: Raw pointer used until transform needed
- **Per-row storage**: Each row gets its own buffer copy
- **Transform idempotent**: Can call on same buffer multiple times

---

## H. PROCESSING GUARANTEES

1. **Every facet key gets a value per row**
   - If not provided: "-" (empty) marker added
   - Ensures consistent output structure

2. **Transformations are deterministic**
   - Same input = same output
   - Safe to call multiple times

3. **Filtering is based on transformed values (if normal transform)**
   - Facets reflect post-transformation state
   - Filter matching works on canonical form

4. **View-only transforms preserve filtering semantics**
   - Filtering based on raw value
   - Display uses transformed form
   - Transparent to end-user filters

5. **Histograms accurately track all data**
   - Normal, unsampled, and estimated all tracked
   - Separate dimensions for data quality visualization

---

## I. RECOMMENDED IMPLEMENTATION PATTERNS

### 1. Normal Transformation (Canonicalize Values)
```c
// Example: Normalize priority levels
facets_register_key_name_transformation(
    facets, "priority", options,
    normalize_priority_callback, NULL  // Not view-only
);

// In callback:
// Input: "WARN", "Warning", "WARNING" -> All output as "warning"
// Effect: Facets show single "warning" option
//         Filtering works on canonical form
```

### 2. View-Only Transformation (Format for Display)
```c
// Example: Add emoji to status
facets_register_key_name_transformation(
    facets, "status",
    options | FACET_KEY_OPTION_TRANSFORM_VIEW,
    add_emoji_callback, NULL  // View-only
);

// In callback:
// Input: "success" -> Display as "✓ success"
// Effect: Raw filtering still works
//         UI shows pretty version
//         Facets show "success" (untransformed)
```

### 3. Faceted Analysis
```c
// Which values to index and make filterable
facets_register_facet(facets, "level", FACET_KEY_OPTION_FACET);

// Make visible in table by default
facets_register_key_name(facets, "message",
    FACET_KEY_OPTION_VISIBLE | FACET_KEY_OPTION_FTS);

// For time-based analysis
facets_set_timeframe_and_histogram_by_name(facets,
    "level",                 // Histogram by priority
    from_usec, to_usec
);
```

---

## J. COMMON PITFALLS

1. **Normal vs View-Only Confusion**
   - Normal: Affects indexing and filtering
   - View-only: Only affects display
   - Choose based on whether you want filtered values affected

2. **Empty Value Semantics**
   - "-" appears when key missing from row
   - Not the same as value being empty string ""
   - Treat separately if distinguishing matters

3. **Transformation Scope Misunderstanding**
   - `FACETS_TRANSFORM_VALUE`: Changes what gets indexed (affects filtering)
   - `FACETS_TRANSFORM_FACET`: Changes UI display (transparent to filtering)
   - Using wrong scope breaks semantic expectations

4. **Histogram Special Values**
   - Unsampled/Estimated appear as separate dimensions
   - Not mixed with normal values
   - Need to handle in UI separately

5. **Counting Logic**
   - `rows_matching_facet_value`: Total occurrences (static)
   - `final_facet_value_counter`: Filtered count (changes with filters)
   - Display should use `final_facet_value_counter`

