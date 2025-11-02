# Tracing Span Lifetime Fix

## Problem

When viewing traces in Jaeger, a single trace would keep growing indefinitely with a duration of many minutes and hundreds of spans. Each function call (`journal_function_call`) was being added as a child to the same long-lived trace instead of creating independent traces.

### Symptoms

- Trace duration: 1m 50s (and growing)
- Total spans: 158 (and increasing)
- All `journal_function_call` spans were children of a single trace
- No clear separation between independent requests

### Root Cause

The `PluginRuntime::run()` method had an `#[instrument]` attribute that created a span lasting the **entire plugin lifetime**. Since this method runs indefinitely until shutdown, all subsequent function calls became children of this never-ending span.

```rust
// BEFORE - Creates a long-lived span
#[instrument(skip_all)]
pub async fn run(mut self) -> Result<()> {
    // ... runs until plugin shutdown
}
```

The trace hierarchy looked like:
```
run (1m 50s+)                          <- Long-lived parent span
├─ declare_functions (388μs)
│  └─ journal_declaration (309μs)
├─ journal_function_call #1 (137ms)    <- Request 1
│  ├─ build_filter_from_selections
│  └─ get_histogram
│     └─ process_histogram_request
├─ journal_function_call #2 (21ms)     <- Request 2
│  └─ ...
├─ journal_function_call #3 (20ms)     <- Request 3
│  └─ ...
└─ ... (continues forever)
```

## Solution

Removed `#[instrument]` from long-lived/setup functions:

1. **`PluginRuntime::run()`** - Runs until plugin shutdown
2. **`PluginRuntime::declare_functions()`** - One-time setup
3. **`Journal::declaration()`** - Metadata generation (no I/O)

### Changes Made

#### 1. rt/src/lib.rs

```rust
// BEFORE
#[instrument(skip_all)]
pub async fn run(mut self) -> Result<()> { ... }

#[instrument(skip_all)]
async fn declare_functions(&self) -> Result<()> { ... }

// AFTER - No instrument macro
pub async fn run(mut self) -> Result<()> { ... }
async fn declare_functions(&self) -> Result<()> { ... }
```

#### 2. log-viewer-plugin/src/main.rs

```rust
// BEFORE
#[instrument(name = "journal_declaration", skip(self))]
fn declaration(&self) -> FunctionDeclaration { ... }

// AFTER - No instrument macro
fn declaration(&self) -> FunctionDeclaration { ... }
```

## Result

Now each function call creates an **independent trace**:

```
journal_function_call (137ms)          <- Independent trace #1
├─ build_filter_from_selections
└─ get_histogram
   └─ process_histogram_request

journal_function_call (21ms)           <- Independent trace #2
├─ build_filter_from_selections
└─ get_histogram
   └─ process_histogram_request

journal_function_call (20ms)           <- Independent trace #3
└─ ...
```

### Benefits

✅ **Independent traces**: Each request is a separate trace with clear boundaries
✅ **Accurate duration**: Trace duration reflects actual request processing time
✅ **Easier analysis**: Can compare individual requests and find outliers
✅ **Proper sampling**: OpenTelemetry sampling works correctly per-request

## Guidelines for Instrumentation

### ✅ DO instrument:

- **Request handlers**: Functions that process external requests
  - `on_call()`, `get_histogram()`, `process_histogram_request()`
- **Short-lived operations**: Functions with bounded execution time
  - Database queries, API calls, computations
- **Business logic**: Functions you want to track individually
  - `build_filter_from_selections()`, indexing operations

### ❌ DON'T instrument:

- **Long-lived functions**: Functions that run indefinitely
  - `run()`, event loops, servers, background tasks
- **Setup/initialization**: One-time startup code
  - `declare_functions()`, configuration loading
- **Pure metadata functions**: Functions that just return data
  - `declaration()`, getters, simple constructors

### Rule of Thumb

If a function's lifetime is:
- **Seconds or less** → Instrument it ✅
- **Minutes or indefinite** → Don't instrument it ❌

## Verification

After the fix, in Jaeger you should see:
- Multiple short traces (100ms - 1s typical)
- Each trace represents one function call
- Clear separation between requests
- No traces with "1m+" durations

Search in Jaeger:
- Service: `log-viewer-plugin`
- Operation: `journal_function_call`
- Look for traces with reasonable durations (< 1s typically)
