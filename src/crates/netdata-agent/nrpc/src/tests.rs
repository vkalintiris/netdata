//! The registry's contracts (C's `nrpc-unittest.c` registry, deletion and catalog suites, where they apply).

use super::*;
use crate::testing::inert;

fn desc(name: &'static [u8], source: Source) -> MethodDesc<'static> {
    MethodDesc {
        name,
        help: b"help",
        tags: b"",
        timeout_s: 10,
        priority: 0,
        version: 0,
        access: 0,
        sync: false,
        source,
        handler: Handler::Builtin(inert),
    }
}

/// The names of what a filter shows, and its DynCfg count.
fn shown(r: &Registry, filter: Filter) -> (Vec<String>, usize) {
    let (methods, dyncfg) = r.visible(filter);
    (methods.into_iter().map(|(k, _)| String::from_utf8(k).unwrap()).collect(), dyncfg)
}

#[test]
fn registration_normalizes_and_replaces_in_place() {
    let r = Registry::default();
    r.register("h", &desc(b"processes", Source::Stream)).unwrap();
    r.register("h", &MethodDesc { tags: b"top,hidden", ..desc(b"b", Source::Stream) }).unwrap();
    r.register("h", &MethodDesc { priority: 5, ..desc(b"processes", Source::Stream) }).unwrap();
    let names: Vec<_> = r.all().into_iter().map(|(k, _)| k).collect();
    assert_eq!(names, [b"processes".to_vec(), b"b".to_vec()]);
    let p = r.get(b"processes").unwrap();
    assert_eq!((p.tags.as_slice(), p.priority, p.flags), (&b"top"[..], 5, 0));
    let b = r.get(b"b").unwrap();
    assert_eq!((b.flags, b.priority), (FLAG_RESTRICTED, PRIORITY_DEFAULT), "priority 0 is the default");
    // a name C would fatal() on is refused (D135.4)
    let long = vec![b'x'; NAME_MAX + 1];
    assert!(r.register("h", &MethodDesc { name: &long, ..desc(b"", Source::Stream) }).is_err());
}

/// A re-registration with changes is C's debug record; an identical one (same thread, epoch, handler) is silent.
#[test]
fn a_changed_reregistration_is_recorded() {
    let r = Registry::default();
    r.register("h", &desc(b"f", Source::Stream)).unwrap();
    let ((), same) = netdata_agent_log::capture(|| r.register("h", &desc(b"f", Source::Stream)).unwrap());
    assert!(same.is_empty(), "{same:?}");
    let ((), changed) = netdata_agent_log::capture(|| {
        r.register("h", &MethodDesc { timeout_s: 20, ..desc(b"f", Source::Stream) }).unwrap()
    });
    let texts: Vec<_> = changed.into_iter().filter_map(|r| r.message).collect();
    assert_eq!(texts, ["NRPC: method 'f' of host h re-registered with changes"]);
}

#[test]
fn dyncfg_names_are_reserved() {
    assert!(name_is_dyncfg(b"config"));
    assert!(name_is_dyncfg(b"config go.d:nginx"));
    assert!(!name_is_dyncfg(b"configure"));
    let r = Registry::default();
    assert_eq!(
        r.register("h", &desc(b"config", Source::Plugin)),
        Err("NRPC: 'host:h' attempted to register reserved dynamic-configuration method 'config' from a plugin. \
             Ignoring it."
            .into())
    );
    r.register("h", &desc(b"config", Source::Stream)).unwrap();
    assert_eq!(r.get(b"config").unwrap().flags, FLAG_DYNCFG);
    assert_eq!(
        r.unregister(b"config", Source::Stream, true),
        Unregistered::Refused("NRPC: refusing to unregister dyncfg method 'config' via FUNCTION_DEL".into())
    );
    assert_eq!(r.unregister(b"config", Source::Daemon, true), Unregistered::Removed { manifest: false });
    assert_eq!(r.unregister(b"config", Source::Daemon, true), Unregistered::NotFound);
    assert!(r.take_pending_dels().is_empty(), "DynCfg is never queued");
    // A name of underscores sanitizes to nothing.
    assert_eq!(
        r.register("h", &desc(b"__", Source::Stream)),
        Err("NRPC: refusing to register method '__' on host 'h': the name sanitizes to an empty string".into())
    );
}

/// A plugin removes only what its own serving thread registered (a restarted run cannot delete its previous run's
/// leftovers); the stream and the daemon remove anything.
#[test]
fn a_plugin_removes_only_its_threads_methods() {
    let r = std::sync::Arc::new(Registry::default());
    {
        let r = std::sync::Arc::clone(&r);
        std::thread::spawn(move || r.register("h", &desc(b"other", Source::Plugin)).unwrap()).join().unwrap();
    }
    let refusal = |by: &str| {
        Unregistered::Refused(format!(
            "NRPC: refusing to unregister method 'other' - serving-thread mismatch (registered by another serving \
             thread, unregister requested by {by})"
        ))
    };
    let r2 = std::sync::Arc::clone(&r);
    let no_handle = std::thread::spawn(move || r2.unregister(b"other", Source::Plugin, false)).join().unwrap();
    assert_eq!(no_handle, refusal("a thread with no serving handle"));
    r.register("h", &desc(b"mine", Source::Plugin)).unwrap();
    assert_eq!(r.unregister(b"other", Source::Plugin, false), refusal("current serving thread"));
    assert_eq!(r.unregister(b"mine", Source::Plugin, false), Unregistered::Removed { manifest: true });
    assert_eq!(r.unregister(b"other", Source::Stream, false), Unregistered::Removed { manifest: true });
}

/// The FUNCTION_DEL journal: a removal is queued while the host has a sender, once per name, in order; a re-add keeps
/// a queued removal; a re-list takes them all.
#[test]
fn removals_are_queued_for_the_parent() {
    let r = Registry::default();
    for name in [&b"a"[..], b"b", b"__hidden"] {
        r.register("h", &MethodDesc { name, ..desc(b"", Source::Stream) }).unwrap();
    }
    assert_eq!(r.unregister(b"b", Source::Stream, true), Unregistered::Removed { manifest: true });
    assert_eq!(r.unregister(b"__hidden", Source::Stream, true), Unregistered::Removed { manifest: false });
    r.register("h", &desc(b"b", Source::Stream)).unwrap();
    assert_eq!(r.unregister(b"b", Source::Stream, true), Unregistered::Removed { manifest: true });
    assert_eq!(r.unregister(b"a", Source::Stream, false), Unregistered::Removed { manifest: true });
    assert_eq!(r.take_pending_dels(), [b"b".to_vec(), b"__hidden".to_vec()]);
    r.register("h", &desc(b"c", Source::Stream)).unwrap();
    assert_eq!(r.unregister(b"c", Source::Stream, true), Unregistered::Removed { manifest: true });
    r.register("h", &desc(b"c", Source::Stream)).unwrap();
    assert_eq!(r.take_pending_dels(), [b"c".to_vec()], "a re-add keeps the queued removal");
    assert!(r.take_pending_dels().is_empty());
}

/// `nrpc_registry_find()`: the whole command, then without its last word; the first available match wins over a
/// longer unavailable one; C's 503 and 404 texts.
#[test]
fn a_command_finds_its_method_as_c() {
    let r = std::sync::Arc::new(Registry::default());
    r.register("h", &desc(b"processes", Source::Stream)).unwrap();
    {
        let r = std::sync::Arc::clone(&r);
        // its thread ends at once: its method is unavailable
        std::thread::spawn(move || r.register("h", &desc(b"processes full", Source::Stream)).unwrap()).join().unwrap();
    }
    let found = |cmd: &[u8]| r.find("h", cmd).map(|m| m.timeout_s);
    let (ok, records) = netdata_agent_log::capture(|| found(b"processes full  info=1"));
    assert_eq!(ok, Ok(10));
    let records: Vec<_> = records.into_iter().filter_map(|r| r.message).collect();
    assert_eq!(records.len(), 1);
    assert!(
        records[0].starts_with("NRPC: method 'processes full  info=1' is not available. host 'h', serving = { tid: ")
            && records[0].ends_with(", running: no }, epoch { owner: 0, stamped: 0 }"),
        "{records:?}"
    );
    assert_eq!(found(b"nothing here"), Err((404, "This feature is not available on this host at this time.")));
    assert_eq!(found(b"processes full"), Ok(10));
    assert!(r.available(b"processes") && !r.available(b"processes full") && !r.available(b"processes x"));
    // a new epoch retires everything registered before it
    r.activate();
    assert_eq!(found(b"processes"), Err((503, "The plugin that registered this feature, is not currently running.")));
    assert!(!r.available(b"processes"));
    r.register("h", &desc(b"processes", Source::Stream)).unwrap();
    assert_eq!(found(b"processes"), Ok(10));
}

/// A command is sanitized with room for hex expansions and a name with none: an invalid byte registers under one key
/// and is called under another, as in C (`nrpc_sanitize_name_dupz()` against the registration's `len + 1`).
#[test]
fn names_and_commands_are_sanitized_with_cs_sizes() {
    assert_eq!(key(b"a\xff"), b"a");
    assert_eq!(sanitize_command(b"a\xff"), b"aff");
    // spaces collapse, double quotes become single ones
    assert_eq!(sanitize_command(b"top  \"x\""), b"top 'x'");
    // only the output is bounded: what follows a long run of spaces still counts (nrpc-internals.h:275-284)
    let long = [&b"fn"[..], &[b' '; FUNCTION_MAX + 4][..], b"arg"].concat();
    assert_eq!(sanitize_command(&long), b"fn arg");
}

/// The catalogs: every filter skips what is unavailable; users see no DynCfg and nothing restricted; a parent sees
/// restricted methods and gets the DynCfg count; registration order.
#[test]
fn the_catalogs_filter_as_c() {
    let r = std::sync::Arc::new(Registry::default());
    for name in [&b"plain"[..], b"__restricted", b"config", b"config go.d:x"] {
        r.register("h", &MethodDesc { name, ..desc(b"", Source::Stream) }).unwrap();
    }
    {
        let r = std::sync::Arc::clone(&r);
        std::thread::spawn(move || r.register("h", &desc(b"gone", Source::Stream)).unwrap()).join().unwrap();
    }
    assert_eq!(shown(&r, Filter::User), (vec!["plain".to_string()], 0));
    assert_eq!(shown(&r, Filter::StreamGlobal), (vec!["plain".to_string(), "__restricted".to_string()], 2));
}

/// C's catalog suite (`nrpc-unittest.c:1428-1708`, its fixtures): the JSON list holds what users see, each with
/// `["GLOBAL"]` (C asserts substrings; the whole string here is C's writer order, `nrpc-catalog.c:183-226`); the export
/// keys each by `"<version>|<name>"` with its help, tags, priority, version and access; the absent-registry block: a
/// host without a registry omits the `functions` key, exports nothing and re-lists nothing.
#[test]
fn the_user_catalogs_render_as_c() {
    use netdata_agent_text::json::{JsonOptions, JsonWriter};
    let r = Registry::default();
    let c = |name, help, tags, timeout_s, priority, version, access| MethodDesc {
        help,
        tags,
        timeout_s,
        priority,
        version,
        access,
        sync: true,
        ..desc(name, Source::Daemon)
    };
    r.register("h", &c(b"c4-global-fn", b"c4 global help", b"top", 11, 42, 3, access::ANONYMOUS_DATA)).unwrap();
    r.register("h", &c(b"__c4-restricted-fn", b"c4 restricted", b"top", 12, 43, 4, 0)).unwrap();
    let dyncfg = c(b"config c4test:job", b"Dynamic configuration", b"config", 120, 1000, 1, access::ANONYMOUS_DATA);
    r.register("h", &dyncfg).unwrap();
    let rendered = |r: &Registry| {
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        catalog::to_json(r, &mut w);
        w.finalize();
        String::from_utf8(w.into_bytes()).unwrap()
    };
    assert_eq!(
        rendered(&r),
        concat!(
            r#"{"functions":{"c4-global-fn":{"help":"c4 global help","timeout":11,"version":3,"options":["GLOBAL"],"#,
            r#""tags":"top","access":["anonymous-data"],"priority":42}}}"#
        )
    );
    let exported = catalog::to_dict(&r);
    assert_eq!(exported.len(), 1);
    let (key, m) = &exported[0];
    assert_eq!(
        (key.as_slice(), m.help.as_slice(), m.tags.as_slice(), m.priority, m.version, m.access),
        (&b"3|c4-global-fn"[..], &b"c4 global help"[..], &b"top"[..], 42, 3, access::ANONYMOUS_DATA)
    );
    // an archived host: no registry, then one again (rrdhost.c:1020, :803)
    r.destroy();
    assert_eq!(rendered(&r), "{}");
    assert!(catalog::to_dict(&r).is_empty() && !r.exists());
    assert_eq!(r.visible(Filter::StreamGlobal).1, 0);
    r.init();
    assert_eq!(rendered(&r), r#"{"functions":{}}"#);
}

/// Without a registry (`nrpc_registry_acquire()` failing, `nrpc-registry.c:525-531`, `nrpc-calls.c:513-517`): a
/// registration is dropped, a call finds nothing (404), and nothing is shown or queued for deletion; `init()` keeps a
/// live registry and gives a destroyed one back empty.
#[test]
fn a_destroyed_registry_registers_nothing() {
    let r = Registry::default();
    r.register("h", &desc(b"kept", Source::Stream)).unwrap();
    r.init();
    assert_eq!(shown(&r, Filter::User), (vec!["kept".to_string()], 0));
    r.destroy();
    r.register("h", &desc(b"late", Source::Stream)).unwrap();
    assert_eq!(r.find("h", b"late").unwrap_err(), (404, "This feature is not available on this host at this time."));
    assert!(!r.available(b"late") && r.all().is_empty());
    assert_eq!(r.unregister(b"late", Source::Daemon, true), Unregistered::NotFound);
    assert!(r.take_pending_dels().is_empty());
    r.init();
    assert_eq!(shown(&r, Filter::User), (Vec::new(), 0));
    r.register("h", &desc(b"again", Source::Stream)).unwrap();
    assert_eq!(shown(&r, Filter::User), (vec!["again".to_string()], 0));
}
