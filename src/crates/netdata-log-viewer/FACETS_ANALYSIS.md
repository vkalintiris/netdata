# Libnetdata Facets Implementation Analysis

## Overview
The libnetdata facets system is a sophisticated framework for querying, filtering, and analyzing log data with support for faceted search, histograms, full-text search, and value transformations.

## A. FACETED DATA INFORMATION STRUCTURE

### 1. Core Data Structures

#### FACET_ROW - Individual Log Entry
```c
typedef struct facet_row {
    usec_t usec;                        // Timestamp in microseconds
    DICTIONARY *dict;                   // Key-value pairs for this row
    FACET_ROW_SEVERITY severity;        // Debug/Normal/Notice/Warning/Critical
    FACET_ROW_BIN_DATA bin_data;        // Binary data attachment (cleanup callback)
    struct facet_row *prev, *next;      // Doubly-linked list pointers
} FACET_ROW;
```

#### FACET_ROW_KEY_VALUE - Value Storage in Row
```c
typedef struct facet_row_key_value {
    const char *tmp;                    // Temporary pointer to raw value
    uint32_t tmp_len;                   // Length of value
    bool empty;                         // Flag: value is empty
    BUFFER *wb;                         // Actual stored buffer (created on insert)
} FACET_ROW_KEY_VALUE;
```

#### FACET_VALUE - Indexed Facet Value Entry
```c
typedef struct facet_value {
    FACETS_HASH hash;                   // XXH3 hash of value
    const char *name;                   // Actual value string
    const char *color;                  // UI color for visualization
    uint32_t name_len;                  // Length of value
    
    bool selected;                      // Is this value selected/filtered?
    bool empty;                         // Special marker for empty values
    bool unsampled;                     // Data was unsampled
    bool estimated;                     // Data was estimated
    
    uint32_t rows_matching_facet_value; // Count of rows with this value
    uint32_t final_facet_value_counter; // Final count after filtering
    uint32_t order;                     // Display order
    
    uint32_t *histogram;                // Time-based histogram buckets
    uint32_t min, max, sum;             // Statistics for the value
    
    struct facet_value *prev, *next;    // Linked list in key
} FACET_VALUE;
```

#### FACET_KEY - Field Definition and Index
```c
struct facet_key {
    FACETS *facets;                     // Back-reference
    FACETS_HASH hash;                   // Hash of field name
    const char *name;                   // Field name
    FACET_KEY_OPTIONS options;          // Configuration flags
    
    bool default_selected_for_values;   // Default filter state
    
    // Current row processing state
    uint32_t key_found_in_row;          // How many times in current row
    uint32_t key_values_selected_in_row; // Count of selected values in row
    uint32_t order;                     // Display order
    
    // Values index: linked list + hashtable
    struct {
        bool enabled;                   // Indexing enabled?
        uint32_t used;                  // Number of unique values
        FACET_VALUE *ll;                // Linked list of values
        SIMPLE_HASHTABLE_VALUE ht;      // Hash table for lookup
    } values;
    
    // Current value being processed
    struct {
        FACETS_HASH hash;               // Hash of raw value
        FACET_KEY_VALUE_FLAGS flags;    // State flags
        const char *raw;                // Raw pointer to value
        uint32_t raw_len;               // Length
        BUFFER *b;                      // Buffer (created on first transform)
        FACET_VALUE *v;                 // Resolved FACET_VALUE
    } current_value;
    
    // Special values (empty, unsampled, estimated)
    struct {
        FACET_VALUE *v;
    } empty_value, unsampled_value, estimated_value;
    
    // Dynamic field callback (for computed fields)
    struct {
        facet_dynamic_row_t cb;
        void *data;
    } dynamic;
    
    // Value transformation callback
    struct {
        bool view_only;                 // Only transform in output
        facets_key_transformer_t cb;
        void *data;
    } transform;
    
    struct facet_key *prev, *next;      // Linked list in facets
};
```

### 2. Special Value Types

The facets system uses special marker values for edge cases:

```c
#define FACET_VALUE_UNSET "-"           // Empty/missing value
#define FACET_VALUE_UNSAMPLED "[unsampled]"  // Data was sampled out
#define FACET_VALUE_ESTIMATED "[estimated]"  // Data was estimated
```

These are stored as pre-defined FACET_VALUE objects:
- **Empty values**: Shown as "-" in output, marked with `empty=true`
- **Unsampled values**: Marked with `unsampled=true`, color="offline"
- **Estimated values**: Marked with `estimated=true`, color="generic"

### 3. FACETS Master Structure

The FACETS structure orchestrates everything:

```c
struct facets {
    // Key and value indexing
    struct {
        size_t count;
        FACET_KEY *ll;                  // Linked list
        SIMPLE_HASHTABLE_KEY ht;        // Hash table for O(1) lookup
    } keys;
    
    // Keys that have facet values enabled
    struct {
        size_t used;
        FACET_KEY *array[FACETS_KEYS_WITH_VALUES_MAX];  // 200 keys max
    } keys_with_values;
    
    // Keys touched in current row (for fast reset)
    struct {
        size_t used;
        FACET_KEY *array[FACETS_KEYS_IN_ROW_MAX];       // 500 keys max
    } keys_in_row;
    
    // Stored rows (doubly-linked list)
    FACET_ROW *base;                    // Head of list
    
    // Filtering
    SIMPLE_PATTERN *query;              // Full-text search pattern
    SIMPLE_PATTERN *visible_keys;       // Which keys to show
    SIMPLE_PATTERN *excluded_keys;      // Which keys to hide
    
    // Pagination with anchors
    struct {
        usec_t start_ut;                // Start timestamp (exclusive boundary)
        usec_t stop_ut;                 // Stop timestamp (exclusive boundary)
        FACETS_ANCHOR_DIRECTION direction; // BACKWARD (new to old) or FORWARD
    } anchor;
    
    // Histogram configuration
    struct {
        FACET_KEY *key;                 // Which field to histogram
        FACETS_HASH hash;               // Field hash
        char *chart;                    // Chart description
        bool enabled;
        uint32_t slots;                 // Number of time buckets
        usec_t slot_width_ut;           // Time per bucket
        usec_t after_ut;                // Histogram start
        usec_t before_ut;               // Histogram end
    } histogram;
    
    // Statistics
    struct {
        struct {
            size_t evaluated;           // Rows processed
            size_t matched;             // Rows kept
            size_t unsampled;
            size_t estimated;
        } rows;
        struct {
            size_t registered;          // Total values seen
            size_t transformed;         // Values transformed
            size_t empty;               // Empty values
            size_t unsampled;
            size_t estimated;
            size_t indexed;             // Values in index
        } values;
    } operations;
};
```

## B. VALUE REMAPPING AND TRANSFORMATION SYSTEM

### 1. Transformation Scope Levels

```c
typedef enum __attribute__((packed)) {
    FACETS_TRANSFORM_VALUE,             // During ingestion/indexing
    FACETS_TRANSFORM_HISTOGRAM,         // When building histogram output
    FACETS_TRANSFORM_FACET,             // When outputting facet options
    FACETS_TRANSFORM_DATA,              // When outputting row data
    FACETS_TRANSFORM_FACET_SORT,        // When sorting facet values
} FACETS_TRANSFORMATION_SCOPE;
```

### 2. Value Processing Pipeline

#### Step 1: Value Ingestion (facets_add_key_value_length)
```
Raw input (key, value, length)
    ↓
Register/find FACET_KEY
    ↓
Store in k->current_value.raw (pointer)
    ↓
Call facets_key_check_value()
```

#### Step 2: Value Transformation (facets_key_check_value)
```
Set FACET_KEY_VALUE_UPDATED flag
    ↓
If transform callback exists AND not view_only:
    • Copy raw to buffer: buffer_contents_replace(k->current_value.b, raw, len)
    • Call transform.cb(facets, buffer, FACETS_TRANSFORM_VALUE, data)
    • Buffer now contains transformed value
    ↓
If full-text search enabled:
    • Copy to buffer if not already copied
    • Match against search pattern
    • Update query match counters
    ↓
If key has facet values enabled:
    • FACET_VALUE_ADD_CURRENT_VALUE_TO_INDEX(k)
    • Hash the (transformed or raw) value
    • Add to values index with deduplication
```

#### Step 3: Value Access (facets_key_get_value)
```c
static inline const char *facets_key_get_value(FACET_KEY *k) {
    return facet_key_value_copied(k) 
        ? buffer_tostring(k->current_value.b)  // Transformed value
        : k->current_value.raw;                 // Raw value
}
```

- If `FACET_KEY_VALUE_COPIED` flag is set, return buffer (transformed value)
- Otherwise return raw pointer (untransformed value)

### 3. Transformation Callback Registration

```c
// Register a transformer for a key
FACET_KEY *facets_register_key_name_transformation(
    FACETS *facets,
    const char *key,
    FACET_KEY_OPTIONS options,
    facets_key_transformer_t cb,        // Callback function
    void *data                          // Context data
);

// Callback signature
typedef void (*facets_key_transformer_t)(
    FACETS *facets,
    BUFFER *wb,                         // Input/output buffer
    FACETS_TRANSFORMATION_SCOPE scope,  // Why are we transforming?
    void *data
);
```

Key feature: The `scope` parameter tells the callback WHY the transformation is happening:
- `FACETS_TRANSFORM_VALUE`: Store the canonical form for indexing
- `FACETS_TRANSFORM_FACET`: Format for facet display
- `FACETS_TRANSFORM_HISTOGRAM`: Format for histogram labels
- `FACETS_TRANSFORM_DATA`: Format for row output
- `FACETS_TRANSFORM_FACET_SORT`: Format for sorting facet values

### 4. View-Only Transformations

```c
// FACET_KEY_OPTION_TRANSFORM_VIEW flag controls timing
if (options & FACET_KEY_OPTION_TRANSFORM_VIEW) {
    // View-only: Transform happens only during output
    // Original value is indexed as-is
    k->transform.view_only = true;
}
```

**Normal transformation** (view_only=false):
1. Transform happens during ingestion → affects indexing
2. Transformed value is what gets indexed and counted
3. Uses FACETS_TRANSFORM_VALUE scope

**View-only transformation** (view_only=true):
1. Raw value is indexed normally
2. Transformation applied only at output time
3. Transparent to faceting/filtering
4. Uses FACETS_TRANSFORM_FACET/FACETS_TRANSFORM_DATA/etc scopes

### 5. Output Transformation (facets_key_value_transformed)

```c
static inline void facets_key_value_transformed(
    FACETS *facets,
    FACET_KEY *k,
    FACET_VALUE *v,
    BUFFER *dst,
    FACETS_TRANSFORMATION_SCOPE scope
) {
    buffer_flush(dst);
    
    // Special values use their hardcoded names
    if (v->empty || v->unsampled || v->estimated)
        buffer_strcat(dst, v->name);        // "-", "[unsampled]", "[estimated]"
    
    // View-only transformations
    else if (k->transform.cb && k->transform.view_only) {
        buffer_contents_replace(dst, v->name, v->name_len);  // Copy value to buffer
        k->transform.cb(facets, dst, scope, k->transform.data); // Transform
    }
    
    // Normal output (either raw or already-transformed from indexing)
    else
        buffer_strcat(dst, facets_key_value_cached(k, v, ...));
}
```

## C. PROCESSING FLOW

### Row Processing Lifecycle

```
facets_rows_begin()
    ↓ Reset all keys for new batch
    
Loop for each row:
    facets_add_key_value(key1, value1)
    facets_add_key_value(key2, value2)
    ...
    
    facets_row_finished(usec)
        ↓
        1. Check FTS filter - reject if no match
        2. Check time range - reject if outside timeframe
        3. Check anchor boundaries - respect pagination direction
        4. For each facet key:
           - If value not provided: set EMPTY value
           - Update value counters
           - Increment histogram slot
        5. Check if all required filters matched
        6. If yes: keep row, increment counters
        7. Reset all keys for next row
```

### Empty Value Handling

When a row finishes but a facet key has no value set:

```c
// In facets_row_finished(), for each key with facet values:
if (\!facet_key_value_updated(k)) {
    facets_key_set_empty_value(facets, k);  // Sets "-" value
}
```

This ensures:
- Every row has a value for every facet key
- Missing values are explicitly tracked
- Facet counts include the empty value

### Special Value Injection

Three special values are automatically managed:

1. **Empty values** (FACET_VALUE_UNSET = "-"):
   - Set when key has no value in row
   - Marked with `empty=true` flag
   - Never filtered from output (unless explicitly excluded)

2. **Unsampled values** (FACET_VALUE_UNSAMPLED = "[unsampled]"):
   - Called via `facets_row_finished_unsampled(facets, usec)`
   - Used when sampling algorithm skips entries
   - Marked with `unsampled=true` flag, color="offline"

3. **Estimated values** (FACET_VALUE_ESTIMATED = "[estimated]"):
   - Called via `facets_update_estimations()`
   - Used for statistical extrapolation above sampling threshold
   - Marked with `estimated=true` flag, color="generic"

## D. FILTERING AND FACETING

### Field Selection Options

```c
typedef enum __attribute__((packed)) {
    FACET_KEY_OPTION_FACET           = (1 << 0),  // Enable as filterable facet
    FACET_KEY_OPTION_NO_FACET        = (1 << 1),  // Not a facet
    FACET_KEY_OPTION_NEVER_FACET     = (1 << 2),  // Never enable as facet
    FACET_KEY_OPTION_STICKY          = (1 << 3),  // Always visible in table
    FACET_KEY_OPTION_VISIBLE         = (1 << 4),  // Default visible in table
    FACET_KEY_OPTION_FTS             = (1 << 5),  // Enable for full-text search
    FACET_KEY_OPTION_MAIN_TEXT       = (1 << 6),  // Full-width text field
    FACET_KEY_OPTION_RICH_TEXT       = (1 << 7),  // Rich formatting support
    FACET_KEY_OPTION_HIDDEN          = (1 << 13), // Don't include in response
    FACET_KEY_OPTION_FILTER_ONLY     = (1 << 14), // Filterable but not exposed
} FACET_KEY_OPTIONS;
```

### Faceting Decision Logic

```c
static inline bool facets_key_is_facet(FACETS *facets, FACET_KEY *k) {
    // Start with default (all keys faceted)
    bool included = facets->all_keys_included_by_default;
    
    // Check explicit options
    if (k->options & FACET_KEY_OPTION_FACET)
        return true;
    
    if (k->options & FACET_KEY_OPTION_NEVER_FACET)
        return false;
    
    // Check patterns if no explicit option
    if (facets->included_keys) {
        if (\!simple_pattern_matches(facets->included_keys, k->name))
            included = false;
    }
    
    if (facets->excluded_keys) {
        if (simple_pattern_matches(facets->excluded_keys, k->name))
            return false;
    }
    
    return included;
}
```

## E. OUTPUT GENERATION

### Report Structure (facets_report)

The output is a comprehensive JSON structure with:

1. **Facets array**: Available filters with value counts
2. **Columns definition**: Field metadata (name, type, transform, visual options)
3. **Data array**: Actual rows with timestamp and values
4. **Histogram**: Time-based distribution data
5. **Statistics**: Processing stats and histogram slots

### Row Output Format

Each row is emitted as a JSON array:
```json
[
    timestamp_usec,
    {
        "severity": "normal|debug|notice|warning|critical"
    },
    value1,
    value2,
    ...
]
```

### Facet Value Output Format

Facets are output with metadata for UI filtering:
```json
{
    "id": "facet_value_id_or_hash",
    "name": "displayed_name",          // After transformation
    "count": 42,                       // Number of matching rows
    "order": 0                         // Display order
}
```

## F. HASHING AND IDENTIFICATION

### Hash Function
```c
#define FACETS_HASH XXH3_64bits(src, len)  // 64-bit XXH3 hash
```

Special hash values:
```c
#define FACETS_HASH_ZERO 0                      // Empty/unset
#define FACETS_HASH_UNSAMPLED UINT64_MAX - 1   // [unsampled] marker
#define FACETS_HASH_ESTIMATED UINT64_MAX        // [estimated] marker
```

### Hash-Based ID Conversion
```c
// Convert 64-bit hash to 11-character alphanumeric string
facets_hash_to_str(hash, char_array);  // "ABCDEFGHIJK" format
```

When `FACETS_OPTION_HASH_IDS` is set, field and value IDs use hashes instead of names.

## G. KEY CHARACTERISTICS

1. **Lazy Indexing**: Values only indexed if field is marked as facet
2. **Dual Storage**: Values stored both as raw pointers and buffers
3. **Callback-Based Transforms**: Extensible via transform callbacks
4. **Scope-Aware Formatting**: Different output formats for different use cases
5. **Out-of-Order Support**: Handles data arriving in any order via anchor system
6. **Sampling & Estimation**: Tracks data quality with special markers
7. **Full-Text Search**: Per-field or global search capability
8. **Histogram Integration**: Automatic time-based aggregation
9. **View-Only Mode**: Transformations that don't affect filtering
10. **Hash-Based Deduplication**: XXH3 hashing for efficient value lookup

