# multicall

A Rust library for creating multi-call binaries, inspired by BusyBox, LLVM, and Rust coreutils.

## What is a Multi-Call Binary?

A multi-call binary is a single executable that provides multiple functionalities based on how it's invoked. Instead of shipping dozens of separate executables, you create symlinks or hardlinks with different names pointing to the same binary. The binary detects which name was used to invoke it and dispatches to the appropriate functionality.

**Benefits:**
- Reduced disk usage (one binary instead of many)
- Smaller distribution packages
- Shared code pages in memory when multiple tools run simultaneously
- Common pattern in embedded systems (BusyBox) and modern toolchains (LLVM)

## Features

- **Zero dependencies** - Uses only the Rust standard library
- **Cross-platform** - Works on Unix and Windows (handles `.exe` extensions automatically)
- **Symlink/hardlink detection** - Automatically detects which program was invoked
- **Prefix support** - Handles prefixed binaries (e.g., `vendor-tool` → `tool`)
- **Alias support** - Create multiple names for the same tool
- **Realpath protection** - Provides `ToolContext` to handle symlink resolution issues
- **Case-sensitive matching** - Unix convention (exact name matching)
- **Comprehensive tests** - Well-tested with examples

## Design Philosophy

This library incorporates best practices from both LLVM and Rust coreutils:

- **From LLVM**: `ToolContext` pattern to handle the realpath problem
- **From Rust coreutils**: Clean prefix detection and alias support
- **Simplified**: No subcommand argument support (only symlink/hardlink invocation)
- **Case-sensitive**: Matches tool names exactly (Unix convention)

## Installation

Add this to your workspace:

```toml
[dependencies]
multicall = { path = "../multicall" }
```

## Usage Example

```rust
use multicall::{MultiCall, ToolContext};
use std::env;

fn main() {
    let mut mc = MultiCall::new();

    // Register your tools
    mc.register("tool1", run_tool1);
    mc.register("tool2", run_tool2);

    // Optional: register aliases
    mc.alias("t1", "tool1");

    // Dispatch based on argv[0]
    let args: Vec<String> = env::args().collect();
    let exit_code = mc.dispatch(&args);
    std::process::exit(exit_code);
}

fn run_tool1(ctx: ToolContext, args: Vec<String>) -> i32 {
    println!("Running tool1 invoked as '{}'", ctx.invocation_name());
    println!("Canonical tool name: '{}'", ctx.tool_name());
    println!("Arguments: {:?}", args);
    0
}

fn run_tool2(_ctx: ToolContext, _args: Vec<String>) -> i32 {
    println!("Running tool2");
    0
}
```

## Creating Symlinks

After building your multi-call binary, create symlinks to it:

**Unix/Linux/macOS:**
```bash
ln -s myapp tool1
ln -s myapp tool2
ln -s myapp t1  # alias
```

**Windows (hardlinks - no admin required):**
```cmd
mklink /H tool1.exe myapp.exe
mklink /H tool2.exe myapp.exe
```

**Windows (symlinks - requires admin):**
```cmd
mklink tool1.exe myapp.exe
mklink tool2.exe myapp.exe
```

## Platform Notes

### Unix/Linux/macOS
- Use symlinks by default (`ln -s`)
- Case-sensitive matching
- Extensions are automatically handled

### Windows
- Automatically handles `.exe` extensions
- Use hardlinks (`mklink /H`) to avoid requiring admin privileges
- Symlinks work but require administrator privileges
- Case-sensitive matching (even though Windows filesystem is case-insensitive)

## Prefix Support

The library supports prefixed binaries where a vendor prefix is separated by a non-alphanumeric character:

**Works:**
- `vendor_tool` → `tool`
- `vendor-tool` → `tool`
- `v.tool` → `tool`

**Doesn't work:**
- `vendortool` → doesn't match `tool` (no separator)

## The Realpath Problem

If your tool calls `realpath()` on `argv[0]`, symlinks will be resolved to the actual binary path, potentially breaking tool detection. The `ToolContext` struct provides protection:

```rust
fn run_tool(ctx: ToolContext, args: Vec<String>) -> i32 {
    if ctx.needs_prepend_arg() {
        // Symlink was resolved - use ctx.tool_name() instead of argv[0]
        println!("Tool name: {}", ctx.tool_name());
    }
    0
}
```

## Limitations

This library **intentionally does not support** the subcommand invocation style like `coreutils ls -l`. It only supports symlink/hardlink-based invocation. This keeps the library simple and focused.

If you need subcommand support, consider using a CLI framework like `clap` directly.

## Running Tests

```bash
cargo test
```

## Comparison with Other Approaches

| Approach | Disk Usage | Memory Usage | Complexity |
|----------|------------|--------------|------------|
| Separate binaries | High | High | Low |
| Multi-call (symlinks) | Low | Low | Medium |
| Multi-call (copies on Windows) | High | Low | Medium |

## Inspiration

This library is inspired by:
- **BusyBox** - The original multi-call binary for embedded Linux
- **LLVM** - Modern toolchain using multi-call pattern
- **Rust coreutils** - Rust reimplementation of GNU coreutils

See `/home/vk/mo/multicall.pdf` for a detailed analysis of these implementations.

## License

Same as your workspace license.
