# Facets Transformation System - Detailed Comparison

## Quick Decision Tree

```
Do you want to change what gets indexed/filtered?
├─ YES → Use NORMAL transformation (view_only=false)
│        Transformation happens at ingestion
│        Filtered values will be the transformed form
│
└─ NO  → Use VIEW-ONLY transformation (FACET_KEY_OPTION_TRANSFORM_VIEW)
         Transformation happens only at output
         Filtering still works on raw values
         Transparent to users
```

## Detailed Comparison Table

| Aspect | Normal Transformation | View-Only Transformation |
|--------|----------------------|-------------------------|
| **When Applied** | During ingestion (facets_key_check_value) | During output only (facets_report) |
| **Affects Indexing** | YES - indexed value is transformed | NO - raw value is indexed |
| **Affects Filtering** | YES - filters match transformed values | NO - filters match raw values |
| **Visible in Facets** | Transformed form | Raw form |
| **Use Cases** | Canonicalize, normalize, deduplicate | Format for display, styling, decoration |
| **Example** | Normalize log levels (WARN→warning) | Add emoji (success→✓ success) |
| **Buffer State** | FACET_KEY_VALUE_COPIED set | FACET_KEY_VALUE_COPIED remains unset |
| **Scope Used** | FACETS_TRANSFORM_VALUE | FACETS_TRANSFORM_FACET / DATA / etc |
| **Performance** | Slight overhead at ingestion | Slight overhead at output only |
| **Semantic Impact** | HIGH - changes meaning for filtering | LOW - cosmetic only |
| **User Expectations** | See transformed values in filters | See raw values in filters, pretty display |

## Side-by-Side Example Scenarios

### Scenario 1: Log Level Normalization

**Goal**: Consolidate different log level formats into single canonical form

**Approach**: Normal transformation

```
Input Data:
  "level": "WARN"    → Transformed to "warning"
  "level": "Warning" → Transformed to "warning"
  "level": "WARNING" → Transformed to "warning"

Indexing:
  All three hash to same value (deduplication)
  Only one facet value: "warning"

Filtering:
  User selects "warning" filter
  Gets all three original formats

Facet Display:
  "warning" (1000 rows)

Output:
  Rows show original format: "WARN", "Warning", "WARNING"
  But facet filtering treats them as one
```

**Implementation**:
```c
facets_register_key_name_transformation(
    facets, "level", FACET_KEY_OPTION_FACET,
    normalize_level, NULL  // view_only NOT set
);
```

### Scenario 2: User-Friendly Status Display

**Goal**: Show emoji/icons while keeping raw filtering

**Approach**: View-only transformation

```
Input Data:
  "status": "success"  → Displayed as "✓ success"
  "status": "error"    → Displayed as "✗ error"
  "status": "pending"  → Displayed as "⏳ pending"

Indexing:
  Raw values indexed: "success", "error", "pending"

Filtering:
  User sees facets: "success", "error", "pending"
  Can filter by raw value names

Facet Display:
  "success" (count: 500)
  "error" (count: 200)
  "pending" (count: 100)

Output Rows:
  "✓ success"    (with emoji)
  "✗ error"      (with emoji)
  "⏳ pending"    (with emoji)
```

**Implementation**:
```c
facets_register_key_name_transformation(
    facets, "status",
    FACET_KEY_OPTION_FACET | FACET_KEY_OPTION_TRANSFORM_VIEW,
    add_status_emoji, NULL  // view_only IS set
);

void add_status_emoji(FACETS *facets, BUFFER *wb,
                      FACETS_TRANSFORMATION_SCOPE scope, void *data) {
    const char *status = buffer_tostring(wb);
    
    // Only transform for output scopes
    if (scope == FACETS_TRANSFORM_FACET ||
        scope == FACETS_TRANSFORM_DATA) {
        
        const char *emoji = NULL;
        if (strcmp(status, "success") == 0) emoji = "✓";
        else if (strcmp(status, "error") == 0) emoji = "✗";
        else if (strcmp(status, "pending") == 0) emoji = "⏳";
        
        buffer_flush(wb);
        if (emoji) buffer_strcat(wb, emoji);
        buffer_strcat(wb, " ");
        buffer_strcat(wb, status);
    }
}
```

### Scenario 3: Multiple Transformations Per Field

**Goal**: Normalize at ingestion AND format for display

**Approach**: Normal transform for canonical form + separate view-only for display

```
Input Data:
  "priority": "critical" → Canonical: "CRIT" → Display: "🔴 CRIT"
  "priority": "high"     → Canonical: "HIGH" → Display: "🟠 HIGH"
  "priority": "medium"   → Canonical: "MED"  → Display: "🟡 MED"
  "priority": "low"      → Canonical: "LOW"  → Display: "🟢 LOW"

Indexing:
  Canonical forms indexed: "CRIT", "HIGH", "MED", "LOW"
  Facets show: "CRIT", "HIGH", "MED", "LOW"

Filtering:
  Users filter by: "CRIT", "HIGH", "MED", "LOW"

Display:
  Rows show: "🔴 CRIT", "🟠 HIGH", "🟡 MED", "🟢 LOW"
  Histogram labels: "🔴 CRIT", "🟠 HIGH", "🟡 MED", "🟢 LOW"
```

**Implementation**:
```c
// First transformation: normalize to canonical form
facets_register_key_name_transformation(
    facets, "priority", FACET_KEY_OPTION_FACET,
    normalize_priority, NULL  // Normal transform (affects indexing)
);

// Second transformation: add emoji (CANNOT BE DONE DIRECTLY)
// View-only transforms apply on top of previous transform
facets_register_key_name_transformation(
    facets, "priority",
    FACET_KEY_OPTION_FACET | FACET_KEY_OPTION_TRANSFORM_VIEW,
    add_priority_emoji, NULL  // View-only transform (display only)
);

void normalize_priority(FACETS *facets, BUFFER *wb,
                       FACETS_TRANSFORMATION_SCOPE scope, void *data) {
    if (scope == FACETS_TRANSFORM_VALUE) {
        const char *value = buffer_tostring(wb);
        buffer_flush(wb);
        
        if (strstr(value, "critical")) buffer_strcat(wb, "CRIT");
        else if (strstr(value, "high")) buffer_strcat(wb, "HIGH");
        else if (strstr(value, "medium")) buffer_strcat(wb, "MED");
        else if (strstr(value, "low")) buffer_strcat(wb, "LOW");
        else buffer_strcat(wb, value);
    }
}

void add_priority_emoji(FACETS *facets, BUFFER *wb,
                       FACETS_TRANSFORMATION_SCOPE scope, void *data) {
    if (scope == FACETS_TRANSFORM_FACET ||
        scope == FACETS_TRANSFORM_DATA ||
        scope == FACETS_TRANSFORM_HISTOGRAM) {
        
        const char *value = buffer_tostring(wb);
        const char *emoji = NULL;
        
        if (strcmp(value, "CRIT") == 0) emoji = "🔴";
        else if (strcmp(value, "HIGH") == 0) emoji = "🟠";
        else if (strcmp(value, "MED") == 0) emoji = "🟡";
        else if (strcmp(value, "LOW") == 0) emoji = "🟢";
        
        if (emoji) {
            buffer_contents_replace(wb, value, strlen(value));
            buffer_prepend(wb, " ");
            buffer_prepend(wb, emoji);
        }
    }
}
```

## Transformation Scope Behavior

### FACETS_TRANSFORM_VALUE
```
When Called:   During facets_key_check_value()
Effect:        Modifies indexed/stored value
Affects:       Indexing, filtering, facet counts
Buffer State:  Sets FACET_KEY_VALUE_COPIED flag
Use For:       Canonicalization, normalization
Called By:     Normal transformations only
```

### FACETS_TRANSFORM_FACET
```
When Called:   When generating facets array in facets_report()
Effect:        Modifies display in filter options
Affects:       Only UI display
Behavior:      View-only transforms called here
Use For:       Formatting facet labels
Example:       "error" → "🔴 error"
```

### FACETS_TRANSFORM_HISTOGRAM
```
When Called:   When generating histogram dimensions
Effect:        Modifies histogram legend labels
Affects:       Only histogram display
Behavior:      View-only transforms called here
Use For:       Formatting dimension names
Example:       "error" → "🔴 Errors"
```

### FACETS_TRANSFORM_DATA
```
When Called:   When outputting row values
Effect:        Modifies cell display in data rows
Affects:       Only row display
Behavior:      View-only transforms called here
Use For:       Cell formatting, styling
Example:       "error" → "<span red>ERROR</span>"
```

### FACETS_TRANSFORM_FACET_SORT
```
When Called:   When sorting facet values
Effect:        Used for comparison during sort
Affects:       Facet value ordering
Behavior:      View-only transforms called here
Use For:       Custom sort order
Example:       "high" → 1, "medium" → 2, "low" → 3
```

## Memory and Performance Implications

### Normal Transformation
```
Ingestion Time:
  + Buffer creation and transformation
  - Indexing is now on transformed value
  - One hash calculation instead of two

Filtering Time:
  - No additional work (value already transformed)

Output Time:
  - No transformation needed
  = Raw pointer retrieval

Total Impact: Slightly slower at ingestion, faster overall
```

### View-Only Transformation
```
Ingestion Time:
  - Hash raw value (fast)
  - Index raw value (fast)
  = No transformation

Filtering Time:
  - No transformation (works on raw)

Output Time:
  + Transform at facet generation
  + Transform at row generation
  + Transform at histogram generation
  = May be called 3+ times per row

Total Impact: Slightly faster at ingestion, more work at output
```

## Decision Criteria

### Use Normal Transformation When:
1. You want different input formats to merge into one value
2. You want filtering to work on canonical form
3. You're canonicalizing/normalizing data
4. You want facet deduplication
5. You don't care about preserving original format in filters

### Use View-Only Transformation When:
1. You want to preserve raw filtering semantics
2. You're adding styling/formatting/decoration
3. Original value is meaningful for filtering
4. You want display to be independent of filtering
5. You want users to see "raw" values in filter options
6. You're adding emoji, colors, or HTML formatting

## Common Mistakes

### Mistake 1: Using Normal Transform for Styling
```c
// WRONG!
facets_register_key_name_transformation(
    facets, "status", 0,
    add_emoji, NULL  // Should have TRANSFORM_VIEW flag
);
// Result: Facet shows emoji, filtering breaks
```

### Mistake 2: Using View-Only for Canonicalization
```c
// WRONG!
facets_register_key_name_transformation(
    facets, "level",
    FACET_KEY_OPTION_TRANSFORM_VIEW,
    normalize_level, NULL  // Should NOT have TRANSFORM_VIEW flag
);
// Result: "WARN", "Warning", "WARNING" show as separate facets
```

### Mistake 3: Forgetting Scope Check
```c
// WRONG!
void my_transform(FACETS *facets, BUFFER *wb,
                  FACETS_TRANSFORMATION_SCOPE scope, void *data) {
    // Always adds emoji, even at FACETS_TRANSFORM_VALUE!
    buffer_prepend(wb, "🔴 ");
}
// Result: Indexing breaks, wrong hash calculated
```

### Mistake 4: Modifying Buffer in View-Only Incorrectly
```c
// WRONG! (if view-only, don't modify during VALUE scope)
void my_transform(FACETS *facets, BUFFER *wb,
                  FACETS_TRANSFORMATION_SCOPE scope, void *data) {
    // Check scope even for view-only!
    if (scope == FACETS_TRANSFORM_VALUE)
        return;  // Don't transform during ingestion
    
    // Now safe to modify for display
    buffer_prepend(wb, "🔴 ");
}
```

## Testing Strategy

### For Normal Transformation
1. Test with different input formats → Should merge into one facet
2. Test filtering → Filters work on canonical form
3. Test counts → All variants counted under canonical value
4. Test output → Shows stored canonical form

### For View-Only Transformation
1. Test with original values → Facets show originals
2. Test filtering → Can filter by original value
3. Test display → Shows formatted version
4. Test counts → Counts based on original value
5. Test multiple scopes → Each scope gets transformation applied

