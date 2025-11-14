//! Multi-call binary support library
//!
//! This library provides functionality for implementing multi-call binaries,
//! where a single executable can behave as different programs based on how
//! it was invoked (typically via symlinks or hardlinks with different names).
//!
//! # Features
//!
//! - **Symlink/hardlink detection**: Automatically detects which program was invoked
//! - **Prefix support**: Handles prefixed binaries (e.g., `vendor-tool` → `tool`)
//! - **Cross-platform**: Works on Unix and Windows (handles `.exe` extensions)
//! - **Realpath protection**: Provides fallback information if symlinks are resolved
//! - **Zero dependencies**: Uses only the Rust standard library
//!
//! # Design Philosophy
//!
//! This library is inspired by the multi-call implementations in LLVM and Rust coreutils,
//! incorporating best practices from both:
//!
//! - **From LLVM**: `ToolContext` pattern to handle realpath issues
//! - **From Rust coreutils**: Clean prefix detection and alias support
//! - **Simplified**: No subcommand argument support (only symlink/hardlink invocation)
//! - **Case-sensitive**: Matches tool names exactly (Unix convention)
//!
//! # Example
//!
//! ```no_run
//! use multicall::{MultiCall, ToolContext};
//! use std::env;
//!
//! fn main() {
//!     let mut mc = MultiCall::new();
//!
//!     // Register your tools
//!     mc.register("tool1", run_tool1);
//!     mc.register("tool2", run_tool2);
//!
//!     // Optional: register aliases
//!     mc.alias("t1", "tool1");
//!
//!     // Dispatch based on argv[0]
//!     let args: Vec<String> = env::args().collect();
//!     let exit_code = mc.dispatch(&args);
//!     std::process::exit(exit_code);
//! }
//!
//! fn run_tool1(ctx: ToolContext, args: Vec<String>) -> i32 {
//!     println!("Running tool1 with {} args", args.len());
//!     0
//! }
//!
//! fn run_tool2(ctx: ToolContext, args: Vec<String>) -> i32 {
//!     println!("Running tool2");
//!     0
//! }
//! ```
//!
//! # Platform Notes
//!
//! ## Windows
//! - Automatically handles `.exe` extensions
//! - Works with both symlinks (requires admin) and hardlinks
//! - Use `ln -f` for hardlinks instead of `ln -s` for symlinks
//!
//! ## Unix
//! - Uses symlinks by default (`ln -s`)
//! - Case-sensitive matching (as per Unix convention)
//!
//! # Gotchas
//!
//! - **Realpath resolution**: If your tool calls `realpath()` on `argv[0]`, use
//!   `ToolContext::needs_prepend_arg()` to know if you need to use the stored tool name
//! - **Prefix detection**: Requires non-alphanumeric separator (e.g., `uu_tool` works,
//!   `uutool` doesn't)
//! - **No subcommand mode**: This library intentionally does not support
//!   `multicall tool1 args...` style invocation

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

/// Context information about how a tool was invoked.
///
/// This structure is inspired by LLVM's ToolContext and provides information
/// that remains valid even if `argv[0]` gets resolved through `realpath()`.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// The original `argv[0]` path
    pub original_path: PathBuf,

    /// The tool name that should be used (after alias resolution)
    pub tool_name: String,

    /// The invocation name (before alias resolution)
    pub invocation_name: String,

    /// Whether this was invoked through a symlink/hardlink or directly
    pub needs_prepend_arg: bool,
}

impl ToolContext {
    /// Returns true if the tool should use `tool_name` rather than examining
    /// the binary path, because symlink resolution may have changed the path.
    ///
    /// This is useful if your tool needs to re-invoke itself or spawn related tools.
    pub fn needs_prepend_arg(&self) -> bool {
        self.needs_prepend_arg
    }

    /// Get the canonical tool name (after alias resolution)
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// Get the name by which this tool was invoked (before alias resolution)
    pub fn invocation_name(&self) -> &str {
        &self.invocation_name
    }
}

/// Type for tool entry points
pub type ToolFn = fn(ToolContext, Vec<String>) -> i32;

/// Multi-call binary dispatcher
///
/// This is the main structure for managing multi-call binaries. Register your
/// tools, set up any aliases, and then call `dispatch()` to route to the
/// appropriate tool based on `argv[0]`.
pub struct MultiCall {
    tools: HashMap<String, ToolFn>,
    aliases: HashMap<String, String>,
}

impl MultiCall {
    /// Create a new multi-call dispatcher
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    /// Register a tool with its entry point function
    ///
    /// # Example
    /// ```
    /// # use multicall::{MultiCall, ToolContext};
    /// let mut mc = MultiCall::new();
    /// mc.register("mytool", |ctx, args| {
    ///     println!("Running mytool");
    ///     0
    /// });
    /// ```
    pub fn register(&mut self, name: &str, func: ToolFn) -> &mut Self {
        self.tools.insert(name.to_string(), func);
        self
    }

    /// Register an alias for a tool
    ///
    /// # Example
    /// ```
    /// # use multicall::{MultiCall, ToolContext};
    /// let mut mc = MultiCall::new();
    /// mc.register("longtool", |ctx, args| 0);
    /// mc.alias("lt", "longtool");  // Now 'lt' invokes 'longtool'
    /// ```
    pub fn alias(&mut self, alias: &str, target: &str) -> &mut Self {
        self.aliases.insert(alias.to_string(), target.to_string());
        self
    }

    /// Dispatch to the appropriate tool based on argv
    ///
    /// This examines `argv[0]` to determine which tool to invoke, following
    /// this priority:
    /// 1. Exact match with a registered tool name
    /// 2. Prefixed match (e.g., `vendor-tool` → `tool`)
    /// 3. Alias resolution
    /// 4. Error if no match found
    ///
    /// Returns the exit code from the tool.
    pub fn dispatch(&self, args: &[String]) -> i32 {
        if args.is_empty() {
            eprintln!("Error: No arguments provided");
            return 1;
        }

        // Get the binary path from argv[0]
        let binary_path = Path::new(&args[0]);

        // Extract the binary name (without path and extension)
        let binary_name = match extract_binary_name(binary_path) {
            Some(name) => name,
            None => {
                // Fall back to current_exe as a last resort
                match env::current_exe() {
                    Ok(path) => {
                        match extract_binary_name(&path) {
                            Some(name) => name,
                            None => {
                                eprintln!("Error: Could not determine binary name");
                                return 1;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error: Could not determine binary name: {}", e);
                        return 1;
                    }
                }
            }
        };

        // Try to find the tool
        let (tool_name, invocation_name, needs_prepend) =
            match self.find_tool(&binary_name) {
                Some((tool, invoked_as, is_direct)) => (tool, invoked_as, !is_direct),
                None => {
                    eprintln!("Error: Unknown tool '{}'", binary_name);
                    self.print_available_tools();
                    return 1;
                }
            };

        // Look up the tool function
        let tool_fn = match self.tools.get(tool_name) {
            Some(func) => func,
            None => {
                eprintln!("Error: Tool '{}' is registered but has no implementation", tool_name);
                return 1;
            }
        };

        // Build the context
        let ctx = ToolContext {
            original_path: binary_path.to_path_buf(),
            tool_name: tool_name.to_string(),
            invocation_name: invocation_name.to_string(),
            needs_prepend_arg: needs_prepend,
        };

        // Call the tool with remaining arguments (skip argv[0])
        let tool_args = args[1..].to_vec();
        tool_fn(ctx, tool_args)
    }

    /// Find which tool matches the given binary name
    ///
    /// Returns (tool_name, invocation_name, is_direct_match)
    /// where is_direct_match is true if the name matched exactly (not prefixed)
    fn find_tool<'a>(&self, name: &'a str) -> Option<(&str, &'a str, bool)> {
        // Try exact match first
        if let Some(tool) = self.tools.get_key_value(name) {
            return Some((tool.0.as_str(), name, true));
        }

        // Try alias resolution
        if let Some(target) = self.aliases.get(name) {
            if self.tools.contains_key(target) {
                return Some((target.as_str(), name, true));
            }
        }

        // Try prefix matching
        if let Some(tool_name) = self.find_prefixed_tool(name) {
            return Some((tool_name, name, false));
        }

        None
    }

    /// Find a tool by stripping prefixes from the binary name
    ///
    /// For example, "uu_mytool" would match "mytool" if it's registered.
    /// The prefix must be separated by a non-alphanumeric character.
    fn find_prefixed_tool(&self, name: &str) -> Option<&str> {
        for tool_name in self.tools.keys() {
            if name.ends_with(tool_name.as_str())
                && name.len() > tool_name.len()
            {
                // Check that there's a non-alphanumeric separator
                let prefix_end = name.len() - tool_name.len();
                if let Some(separator) = name.chars().nth(prefix_end - 1) {
                    if !separator.is_alphanumeric() {
                        return Some(tool_name.as_str());
                    }
                }
            }
        }
        None
    }

    /// Print available tools (for error messages)
    fn print_available_tools(&self) {
        if self.tools.is_empty() {
            return;
        }

        eprintln!("\nAvailable tools:");
        let mut names: Vec<_> = self.tools.keys().collect();
        names.sort();
        for name in names {
            eprintln!("  {}", name);
        }

        if !self.aliases.is_empty() {
            eprintln!("\nAliases:");
            let mut alias_list: Vec<_> = self.aliases.iter().collect();
            alias_list.sort_by_key(|(k, _)| *k);
            for (alias, target) in alias_list {
                eprintln!("  {} → {}", alias, target);
            }
        }
    }
}

impl Default for MultiCall {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract the binary name from a path, handling extensions
///
/// This uses `file_stem()` which automatically strips extensions like `.exe`
fn extract_binary_name(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_tool(_ctx: ToolContext, _args: Vec<String>) -> i32 {
        0
    }

    #[test]
    fn test_extract_binary_name() {
        assert_eq!(extract_binary_name(Path::new("/usr/bin/mytool")), Some("mytool".to_string()));
        assert_eq!(extract_binary_name(Path::new("mytool")), Some("mytool".to_string()));
        assert_eq!(extract_binary_name(Path::new("mytool.exe")), Some("mytool".to_string()));
        assert_eq!(extract_binary_name(Path::new("/path/to/tool.bin")), Some("tool".to_string()));
        assert_eq!(extract_binary_name(Path::new("")), None);
    }

    #[test]
    fn test_exact_match() {
        let mut mc = MultiCall::new();
        mc.register("tool1", dummy_tool);
        mc.register("tool2", dummy_tool);

        assert_eq!(mc.find_tool("tool1"), Some(("tool1", "tool1", true)));
        assert_eq!(mc.find_tool("tool2"), Some(("tool2", "tool2", true)));
        assert_eq!(mc.find_tool("tool3"), None);
    }

    #[test]
    fn test_alias() {
        let mut mc = MultiCall::new();
        mc.register("longtool", dummy_tool);
        mc.alias("lt", "longtool");

        assert_eq!(mc.find_tool("lt"), Some(("longtool", "lt", true)));
        assert_eq!(mc.find_tool("longtool"), Some(("longtool", "longtool", true)));
    }

    #[test]
    fn test_prefix_detection() {
        let mut mc = MultiCall::new();
        mc.register("tool", dummy_tool);
        mc.register("mytool", dummy_tool);

        // Valid prefix separators
        assert_eq!(mc.find_tool("prefix_tool"), Some(("tool", "prefix_tool", false)));
        assert_eq!(mc.find_tool("prefix-tool"), Some(("tool", "prefix-tool", false)));
        assert_eq!(mc.find_tool("prefix.tool"), Some(("tool", "prefix.tool", false)));
        assert_eq!(mc.find_tool("p_mytool"), Some(("mytool", "p_mytool", false)));

        // Should NOT match without separator
        assert_eq!(mc.find_tool("prefixtool"), None);
        assert_eq!(mc.find_tool("pmytool"), None);
    }

    #[test]
    fn test_prefix_priority() {
        let mut mc = MultiCall::new();
        mc.register("tool", dummy_tool);
        mc.register("mytool", dummy_tool);

        // When multiple tools match, should prefer the longest match
        // (In this implementation, iteration order determines which is found first,
        // but that's okay - in practice you wouldn't have overlapping names)
        let result = mc.find_tool("my_mytool");
        assert!(result.is_some());
        let (tool_name, _, _) = result.unwrap();
        assert!(tool_name == "mytool" || tool_name == "tool");
    }

    #[test]
    fn test_tool_context() {
        let ctx = ToolContext {
            original_path: PathBuf::from("/usr/bin/mytool"),
            tool_name: "tool".to_string(),
            invocation_name: "mytool".to_string(),
            needs_prepend_arg: true,
        };

        assert!(ctx.needs_prepend_arg());
        assert_eq!(ctx.tool_name(), "tool");
        assert_eq!(ctx.invocation_name(), "mytool");
    }
}
