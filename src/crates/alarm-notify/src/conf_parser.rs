//! Reader for `health_alarm_notify.conf`.
//!
//! The file used to be `source`d by bash, so it is not a declarative format: it is
//! shell. Rather than invent a new format and break every existing installation, we
//! parse the subset that real configurations use - assignments with the three
//! quoting styles, `${VAR}` references, string-keyed arrays, `unset`, comments and line
//! continuations - and report anything we cannot handle instead of silently
//! ignoring it.
//!
//! Command substitution is the one construct we cannot evaluate ourselves. Where a
//! POSIX shell exists we hand just that fragment to it; where none does (Windows)
//! it expands to empty with a warning, which is what bash would also produce for a
//! command that cannot run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Everything a config file contributed.
#[derive(Debug, Default)]
pub struct ConfigData {
    /// Scalar variables, last assignment wins.
    pub vars: HashMap<String, String>,
    /// `name[key]=value` tables, e.g. `role_recipients_email[sysadmin]`.
    pub arrays: HashMap<String, HashMap<String, String>>,
    /// Body of a `custom_sender()` shell function, if the user defined one.
    pub custom_sender_body: Option<String>,
    /// Files that were actually read, in load order.
    pub loaded_files: Vec<PathBuf>,
    /// Constructs we could not interpret: `(file, line number, text)`.
    pub unsupported: Vec<(PathBuf, usize, String)>,
}

impl ConfigData {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }

    /// Value or empty string - the shell treated unset and empty identically.
    pub fn str(&self, key: &str) -> &str {
        self.get(key).unwrap_or("")
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.vars.insert(key.to_string(), value.to_string());
    }

    pub fn array(&self, name: &str) -> Option<&HashMap<String, String>> {
        self.arrays.get(name)
    }

    /// Merge another file's contribution on top of this one.
    pub fn merge(&mut self, other: ConfigData) {
        self.vars.extend(other.vars);
        for (name, entries) in other.arrays {
            self.arrays.entry(name).or_default().extend(entries);
        }
        if other.custom_sender_body.is_some() {
            self.custom_sender_body = other.custom_sender_body;
        }
        self.loaded_files.extend(other.loaded_files);
        self.unsupported.extend(other.unsupported);
    }
}

/// Parse one config file, expanding references against `seed` (already-known
/// variables) and the process environment.
pub fn parse_file(path: &Path, seed: &HashMap<String, String>) -> std::io::Result<ConfigData> {
    let raw = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    Ok(parse_str(&text, path, seed))
}

pub fn parse_str(text: &str, path: &Path, seed: &HashMap<String, String>) -> ConfigData {
    let mut data = ConfigData {
        loaded_files: vec![path.to_path_buf()],
        ..Default::default()
    };
    // Expansion scope: what we have learned so far, on top of the seed.
    let mut scope = seed.clone();

    let lines: Vec<&str> = text.split('\n').collect();
    let mut idx = 0usize;
    while idx < lines.len() {
        let line_no = idx + 1;
        let raw_line = lines[idx];
        idx += 1;

        let trimmed = raw_line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // A shell function definition. `custom_sender()` is captured verbatim so the
        // shim can re-source the user's file and call it; any other function is
        // recorded as unsupported.
        if let Some(name) = function_header(trimmed) {
            let (body, consumed) = capture_function_body(&lines, idx - 1);
            idx = consumed;
            if name == "custom_sender" {
                data.custom_sender_body = Some(body);
            } else {
                data.unsupported
                    .push((path.to_path_buf(), line_no, format!("{name}() function")));
            }
            continue;
        }

        // Join line continuations before looking at the statement.
        let mut statement = String::new();
        let mut cur = raw_line;
        loop {
            if let Some(stripped) = cur.strip_suffix('\\') {
                // Backslash-newline is line joining: both characters disappear.
                statement.push_str(stripped);
                if idx < lines.len() {
                    cur = lines[idx];
                    idx += 1;
                    continue;
                }
            } else {
                statement.push_str(cur);
            }
            break;
        }
        // A double-quoted value may legitimately span lines.
        while unbalanced_quotes(&statement) && idx < lines.len() {
            statement.push('\n');
            statement.push_str(lines[idx]);
            idx += 1;
        }

        let stmt = strip_leading_keywords(statement.trim());
        if stmt.is_empty() {
            continue;
        }

        if let Some(rest) = stmt.strip_prefix("unset ") {
            for name in rest.split_whitespace() {
                let name = name.trim_start_matches("-v").trim();
                if name.is_empty() {
                    continue;
                }
                data.vars.remove(name);
                scope.remove(name);
            }
            continue;
        }

        match split_assignment(stmt) {
            Some((target, raw_value)) => {
                let (value, mut problems) = expand_value(raw_value, &scope);
                match target {
                    Target::Scalar(name) => {
                        scope.insert(name.clone(), value.clone());
                        data.vars.insert(name, value);
                    }
                    Target::Element { name, key } => {
                        let (key, key_problems) = expand_value(&key, &scope);
                        problems.extend(key_problems);
                        data.arrays.entry(name).or_default().insert(key, value);
                    }
                }
                for p in problems {
                    data.unsupported.push((path.to_path_buf(), line_no, p));
                }
            }
            None => {
                // `declare -A x` and similar declarations carry no value; they are
                // implicit in our data model and safe to skip quietly.
                if !is_ignorable_declaration(stmt) {
                    data.unsupported
                        .push((path.to_path_buf(), line_no, stmt.to_string()));
                }
            }
        }
    }

    data
}

enum Target {
    Scalar(String),
    Element { name: String, key: String },
}

fn strip_leading_keywords(mut s: &str) -> &str {
    for kw in ["export ", "declare ", "typeset ", "local ", "readonly "] {
        if let Some(rest) = s.strip_prefix(kw) {
            let rest = rest.trim_start();
            // `declare -A name` / `declare -a name` keep their flag; leave it for
            // the ignorable-declaration check.
            s = rest;
        }
    }
    s
}

fn is_ignorable_declaration(s: &str) -> bool {
    // `-A name`, `-a name`, `-g name`, or a bare name with no value.
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('-') {
        return rest
            .split_whitespace()
            .next()
            .is_some_and(|flags| flags.chars().all(|c| c.is_ascii_alphabetic()));
    }
    !s.contains('=') && s.split_whitespace().count() == 1 && is_identifier(s)
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn function_header(s: &str) -> Option<&str> {
    let s = s.trim();
    let s = s.strip_prefix("function ").map(str::trim).unwrap_or(s);
    let (name, rest) = s.split_once('(')?;
    let rest = rest.trim_start();
    if !rest.starts_with(')') {
        return None;
    }
    let name = name.trim();
    if is_identifier(name) {
        Some(name)
    } else {
        None
    }
}

/// Collect a function body by brace depth, returning it and the index just past it.
fn capture_function_body(lines: &[&str], start: usize) -> (String, usize) {
    let mut body = String::new();
    let mut depth = 0usize;
    let mut started = false;
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        body.push_str(line);
        body.push('\n');
        for c in line.chars() {
            match c {
                '{' => {
                    depth += 1;
                    started = true;
                }
                '}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        i += 1;
        if started && depth == 0 {
            break;
        }
    }
    (body, i)
}

fn unbalanced_quotes(s: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if in_double => {
                chars.next();
            }
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => break,
            _ => {}
        }
    }
    in_single || in_double
}

/// Split `NAME=value` or `NAME[key]=value`, rejecting anything that is not an
/// assignment (a command, a conditional, ...).
fn split_assignment(stmt: &str) -> Option<(Target, &str)> {
    let eq = find_assignment_eq(stmt)?;
    let (lhs, rhs) = (&stmt[..eq], &stmt[eq + 1..]);
    // `NAME+=value` appends in bash. These keys are only ever set once in practice,
    // so the name is taken and the last assignment wins.
    let lhs = lhs.strip_suffix('+').unwrap_or(lhs);
    if let Some(open) = lhs.find('[') {
        let name = &lhs[..open];
        let key = lhs[open + 1..].strip_suffix(']')?;
        if !is_identifier(name) {
            return None;
        }
        return Some((
            Target::Element {
                name: name.to_string(),
                key: key.to_string(),
            },
            rhs,
        ));
    }
    if !is_identifier(lhs) {
        return None;
    }
    Some((Target::Scalar(lhs.to_string()), rhs))
}

fn find_assignment_eq(stmt: &str) -> Option<usize> {
    let bytes = stmt.as_bytes();
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'[' => depth += 1,
            b']' => depth = depth.saturating_sub(1),
            b'=' if depth == 0 => return Some(i),
            // Whitespace before any `=` means this is a command, not an assignment.
            b' ' | b'\t' => return None,
            _ => {}
        }
    }
    None
}

/// Expand one right-hand side. Returns the value plus any unsupported constructs
/// encountered.
fn expand_value(raw: &str, scope: &HashMap<String, String>) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut problems = Vec::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        match chars[i] {
            '\'' => {
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    out.push(chars[i]);
                    i += 1;
                }
                i += 1; // closing quote
            }
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        let next = chars[i + 1];
                        if matches!(next, '"' | '\\' | '$' | '`') {
                            out.push(next);
                            i += 2;
                            continue;
                        }
                        if next == '\n' {
                            i += 2;
                            continue;
                        }
                    }
                    if chars[i] == '$' {
                        i = expand_dollar(&chars, i, scope, &mut out, &mut problems);
                        continue;
                    }
                    if chars[i] == '`' {
                        i = expand_backtick(&chars, i, &mut out, &mut problems);
                        continue;
                    }
                    out.push(chars[i]);
                    i += 1;
                }
                i += 1; // closing quote
            }
            '\\' if i + 1 < chars.len() => {
                if chars[i + 1] != '\n' {
                    out.push(chars[i + 1]);
                }
                i += 2;
            }
            '$' => {
                i = expand_dollar(&chars, i, scope, &mut out, &mut problems);
            }
            '`' => {
                i = expand_backtick(&chars, i, &mut out, &mut problems);
            }
            c if c.is_whitespace() => break,
            c => {
                out.push(c);
                i += 1;
            }
        }
    }

    (out, problems)
}

/// Handle `$VAR`, `${VAR}`, `${VAR:-default}`, `${VAR:+alt}` and `$(command)`.
fn expand_dollar(
    chars: &[char],
    at: usize,
    scope: &HashMap<String, String>,
    out: &mut String,
    problems: &mut Vec<String>,
) -> usize {
    let mut i = at + 1;
    if i >= chars.len() {
        out.push('$');
        return i;
    }

    if chars[i] == '(' {
        // `$((arith))` is not supported; `$(command)` is delegated to a shell.
        if chars.get(i + 1) == Some(&'(') {
            let (inner, next) = balanced(chars, i, '(', ')');
            problems.push(format!("arithmetic expansion $({inner})"));
            return next;
        }
        let (inner, next) = balanced(chars, i, '(', ')');
        out.push_str(&run_command_substitution(&inner, problems));
        return next;
    }

    if chars[i] == '{' {
        let (inner, next) = balanced(chars, i, '{', '}');
        out.push_str(&expand_braced(&inner, scope, problems));
        return next;
    }

    // Bare `$NAME`.
    let start = i;
    while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
        i += 1;
    }
    if i == start {
        out.push('$');
        return i;
    }
    let name: String = chars[start..i].iter().collect();
    out.push_str(lookup(&name, scope).as_deref().unwrap_or(""));
    i
}

fn expand_backtick(
    chars: &[char],
    at: usize,
    out: &mut String,
    problems: &mut Vec<String>,
) -> usize {
    let mut i = at + 1;
    let mut inner = String::new();
    while i < chars.len() && chars[i] != '`' {
        inner.push(chars[i]);
        i += 1;
    }
    out.push_str(&run_command_substitution(&inner, problems));
    i + 1
}

fn expand_braced(
    inner: &str,
    scope: &HashMap<String, String>,
    problems: &mut Vec<String>,
) -> String {
    // `${VAR:-default}` / `${VAR-default}`
    for sep in [":-", "-"] {
        if let Some((name, default)) = inner.split_once(sep) {
            if is_identifier(name) {
                let cur = lookup(name, scope).unwrap_or_default();
                let empty_counts = sep == ":-";
                return if cur.is_empty() && empty_counts || lookup(name, scope).is_none() {
                    let (v, mut p) = expand_value(default, scope);
                    problems.append(&mut p);
                    v
                } else {
                    cur
                };
            }
        }
    }
    // `${VAR:+alt}`
    if let Some((name, alt)) = inner.split_once(":+") {
        if is_identifier(name) {
            let cur = lookup(name, scope).unwrap_or_default();
            if cur.is_empty() {
                return String::new();
            }
            let (v, mut p) = expand_value(alt, scope);
            problems.append(&mut p);
            return v;
        }
    }
    if is_identifier(inner) {
        return lookup(inner, scope).unwrap_or_default();
    }
    problems.push(format!("parameter expansion ${{{inner}}}"));
    String::new()
}

fn lookup(name: &str, scope: &HashMap<String, String>) -> Option<String> {
    if let Some(v) = scope
        .get(name)
        .cloned()
        .or_else(|| std::env::var(name).ok())
    {
        return Some(v);
    }

    // A reference to an alert variable is not an unset variable: it is a template to
    // resolve once the alert is known. Hand the placeholder back untouched.
    if crate::message::is_runtime_variable(name) {
        return Some(format!("${{{name}}}"));
    }

    None
}

/// Consume a `(...)`/`{...}` group starting at `chars[at]`, returning its interior
/// and the index just past the closing delimiter.
fn balanced(chars: &[char], at: usize, open: char, close: char) -> (String, usize) {
    let mut depth = 0usize;
    let mut inner = String::new();
    let mut i = at;
    while i < chars.len() {
        let c = chars[i];
        if c == open {
            depth += 1;
            i += 1;
            if depth == 1 {
                continue;
            }
        } else if c == close {
            depth -= 1;
            i += 1;
            if depth == 0 {
                break;
            }
            inner.push(c);
            continue;
        } else {
            i += 1;
        }
        inner.push(c);
    }
    (inner, i)
}

/// Run `$(...)` through a POSIX shell, mirroring bash: trailing newlines are
/// stripped, and a command that cannot run yields an empty string.
fn run_command_substitution(inner: &str, problems: &mut Vec<String>) -> String {
    let shell = match crate::exec::posix_shell() {
        Some(sh) => sh,
        None => {
            // Reported at error level: the same config file then means different
            // things on different platforms, and the operator must know which.
            problems.push(format!(
                "command substitution $({inner}) needs a POSIX shell, which is unavailable; it expanded to nothing"
            ));
            return String::new();
        }
    };
    match Command::new(&shell).arg("-c").arg(inner).output() {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.trim_end_matches('\n').to_string()
        }
        Ok(_) => {
            problems.push(format!("command substitution $({inner}) failed"));
            String::new()
        }
        Err(e) => {
            problems.push(format!(
                "command substitution $({inner}) could not run: {e}"
            ));
            String::new()
        }
    }
}

#[cfg(test)]
#[path = "conf_parser_tests.rs"]
mod tests;
