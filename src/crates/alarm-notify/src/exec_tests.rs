use super::*;

#[test]
#[cfg(unix)]
fn which_finds_a_standard_tool_and_rejects_nonsense() {
    assert!(which("sh").is_some());
    assert!(which("definitely-not-a-real-program-xyzzy").is_none());
}

#[test]
#[cfg(unix)]
fn absolute_paths_are_checked_directly() {
    assert!(which("/bin/sh").is_some() || which("/usr/bin/sh").is_some());
    assert!(which("/nonexistent/prog").is_none());
}

#[test]
#[cfg(unix)]
fn posix_shell_is_discoverable_here() {
    assert!(posix_shell().is_some());
}

#[test]
#[cfg(unix)]
fn run_captures_status_and_streams() {
    let sh = posix_shell().unwrap();
    let out = run(&sh, ["-c", "printf out; printf err >&2; exit 3"], None, &[]).unwrap();
    assert_eq!(out.status, Some(3));
    assert!(!out.success());
    assert_eq!(out.stdout, "out");
    assert_eq!(out.stderr, "err");
    assert_eq!(out.combined(), "out err");
}

#[test]
#[cfg(unix)]
fn run_writes_stdin() {
    let sh = posix_shell().unwrap();
    let out = run(&sh, ["-c", "cat"], Some(b"payload"), &[]).unwrap();
    assert!(out.success());
    assert_eq!(out.stdout, "payload");
}

#[test]
#[cfg(unix)]
fn run_passes_environment() {
    let sh = posix_shell().unwrap();
    let env = vec![("ND_TEST_VAR".to_string(), "42".to_string())];
    let out = run(&sh, ["-c", "printf %s \"$ND_TEST_VAR\""], None, &env).unwrap();
    assert_eq!(out.stdout, "42");
}
