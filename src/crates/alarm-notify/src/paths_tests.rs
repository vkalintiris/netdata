use super::*;

fn paths() -> Paths {
    Paths {
        user_config_dir: "/etc/netdata".into(),
        stock_config_dir: "/usr/lib/netdata/conf.d".into(),
        cache_dir: "/var/cache/netdata".into(),
        registry_dir: "/var/lib/netdata/registry".into(),
    }
}

#[test]
fn tracking_state_stays_inside_the_cache_directory() {
    let p = paths();
    // The ordinary case.
    assert_eq!(
        p.criticality_tracking_dir("email", "root"),
        PathBuf::from("/var/cache/netdata/alarm-notify/email/root")
    );
    // An absolute recipient must not replace the prefix.
    let escaped = p.criticality_tracking_dir("email", "/tmp/pwned");
    assert!(
        escaped.starts_with("/var/cache/netdata/alarm-notify"),
        "{escaped:?}"
    );
    // Nor may a traversal: the separators are folded, so it stays one component.
    let traversed = p.criticality_tracking_dir("email", "../../../etc");
    assert_eq!(
        traversed,
        PathBuf::from("/var/cache/netdata/alarm-notify/email/.._.._.._etc")
    );
    assert_eq!(
        traversed.components().count(),
        PathBuf::from("/var/cache/netdata/alarm-notify/email/x")
            .components()
            .count()
    );
}

#[test]
fn path_components_are_reduced_but_still_readable() {
    assert_eq!(sanitize_path_component("root"), "root");
    assert_eq!(sanitize_path_component("#alerts"), "#alerts");
    assert_eq!(sanitize_path_component("a/b"), "a_b");
    assert_eq!(sanitize_path_component(".."), "_");
    assert_eq!(sanitize_path_component(""), "_");
}
