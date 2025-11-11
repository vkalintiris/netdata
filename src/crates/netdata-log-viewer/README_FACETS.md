# Libnetdata Facets System - Documentation Index

This directory contains comprehensive documentation of the libnetdata facets implementation, explored from `/home/vk/repos/nd/master/src/libnetdata/facets/`.

## Documentation Files

### 1. **FACETS_KEY_FINDINGS.md** - START HERE
**Best for**: Quick understanding and practical implementation
- What faceted data emits (5 major components)
- Transformation points and modes (Normal vs View-Only)
- Special value markers (empty, unsampled, estimated)
- Counting semantics explained
- Field options reference
- Common pitfalls and patterns

### 2. **FACETS_ANALYSIS.md** - COMPREHENSIVE REFERENCE
**Best for**: Deep technical understanding
- Core data structures with annotations
  - FACET_ROW, FACET_ROW_KEY_VALUE, FACET_VALUE, FACET_KEY
  - Special value types and their handling
  - FACETS master orchestration structure
- Value transformation system in detail
  - Transformation scope levels
  - Complete processing pipeline
  - Callback registration and execution
  - View-only transformation mechanics
- Processing flow and lifecycle
  - Row processing lifecycle
  - Empty value handling
  - Special value injection
- Filtering and faceting logic
  - Field selection options
  - Faceting decision algorithm
- Output generation process
  - Report structure
  - Row/facet/histogram output formats
- Hashing and identification system
- 10 key characteristics summary

### 3. **FACETS_DATA_FLOW.txt** - VISUAL REFERENCE
**Best for**: Understanding the processing pipeline
- Data ingestion stage (step-by-step flow)
- Row completion stage with filtering logic
- Output generation in 4 parts
  - Facets array generation
  - Columns definition
  - Data rows output
  - Histogram generation
- Special value handling
- Transformation scopes
- Buffer lifecycle
- Counting logic
- Hash-based ID encoding

## Key Concepts at a Glance

### Five Transformation Scopes
```
FACETS_TRANSFORM_VALUE   → During ingestion, affects indexing
FACETS_TRANSFORM_FACET   → Filter display, doesn't affect filtering
FACETS_TRANSFORM_HISTOGRAM → Legend labels
FACETS_TRANSFORM_DATA    → Row data display
FACETS_TRANSFORM_FACET_SORT → Custom sorting
```

### Two Transformation Modes
```
Normal (view_only=false)  → Transforms indexed values, affects filters
View-Only (TRANSFORM_VIEW) → Only affects display, transparent to filters
```

### Three Special Values
```
"-"           (empty)     → Key missing from row
"[unsampled]"  → Data skipped by sampling algorithm
"[estimated]"  → Data statistically extrapolated
```

### Output Components
```
1. Facets array     - Filter options with counts
2. Columns          - Field schema and metadata
3. Data rows        - Actual log entries with values
4. Histogram        - Time-based distribution
5. Statistics       - Query metadata and performance
```

## Source Code References

All documentation derived from:
- `/home/vk/repos/nd/master/src/libnetdata/facets/facets.h` - API and structures
- `/home/vk/repos/nd/master/src/libnetdata/facets/facets.c` - Implementation (2969 lines)
- `/home/vk/repos/nd/master/src/libnetdata/facets/README.md` - Original documentation

## Key Implementation Details

### Data Structures
| Structure | Purpose | Size/Limit |
|-----------|---------|-----------|
| FACETS | Main orchestrator | 1 per query |
| FACET_KEY | Field definition | Unlimited |
| FACET_VALUE | Value entry | Unlimited per key |
| FACET_ROW | Log entry | Configurable max |
| FACET_ROW_KEY_VALUE | Value storage in row | 1 per key per row |

### Processing Limits
```c
#define FACETS_HISTOGRAM_COLUMNS 150        // Max histogram time slots
#define FACETS_KEYS_WITH_VALUES_MAX 200     // Max facetable keys
#define FACETS_KEYS_IN_ROW_MAX 500          // Max keys touched per row
```

### Hash System
```c
Hash Function: XXH3_64bits (64-bit)
Encoding: 11-character alphanumeric string (6 bits per char)
Special values:
  - HASH_ZERO = 0 (empty/unset)
  - HASH_UNSAMPLED = UINT64_MAX - 1
  - HASH_ESTIMATED = UINT64_MAX
```

## Common Tasks

### Understanding Row Processing
1. Read: FACETS_DATA_FLOW.txt (Stage 2)
2. Details: FACETS_ANALYSIS.md (Section C)
3. Practical: FACETS_KEY_FINDINGS.md (Section D)

### Implementing Transformations
1. Quick start: FACETS_KEY_FINDINGS.md (Section I)
2. Technical: FACETS_ANALYSIS.md (Section B)
3. Scopes: FACETS_DATA_FLOW.txt (Stage 5)

### Understanding Output Format
1. Overview: FACETS_KEY_FINDINGS.md (Section A)
2. Details: FACETS_ANALYSIS.md (Section E)
3. Generation: FACETS_DATA_FLOW.txt (Stage 3)

### Debugging Value Issues
1. Flow: FACETS_DATA_FLOW.txt (Stage 1)
2. Structures: FACETS_ANALYSIS.md (Section A)
3. Counting: FACETS_KEY_FINDINGS.md (Section D)

## Critical Points to Remember

1. **Transformation timing matters**: Normal transforms affect indexing; view-only transforms only affect display.

2. **Every key gets a value**: Missing values are marked as "-" (empty), ensuring consistent row structure.

3. **Two counting semantics**: `rows_matching_facet_value` (total) vs `final_facet_value_counter` (filtered).

4. **Special values not transformed**: Empty, unsampled, and estimated values use hardcoded strings.

5. **Buffer lazy allocation**: Values use raw pointers until transformation is needed.

6. **Histogram includes all states**: Normal, unsampled, and estimated values appear as separate dimensions.

## Implementation Checklist

When implementing facets integration:

- [ ] Understand output format (5 components)
- [ ] Plan transformation strategy (normal vs view-only)
- [ ] Register field options appropriately
- [ ] Handle special values correctly
- [ ] Consider filtering semantics (raw vs transformed)
- [ ] Plan histogram generation if needed
- [ ] Test with empty/missing values
- [ ] Verify counting logic
- [ ] Check buffer lifecycle
- [ ] Plan performance for large datasets

---

Generated: November 2025
Source: libnetdata facets system (master branch)
Documentation scope: Very thorough
