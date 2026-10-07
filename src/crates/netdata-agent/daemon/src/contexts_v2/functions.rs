//! The functions dictionary of the contexts v2 engine: its insert and conflict callbacks and its writer
//! (`src/database/contexts/api_v2_contexts.c`).

use std::collections::HashMap;
use std::sync::Arc;

use netdata_agent_nrpc::{Method, access};
use netdata_agent_text::json::JsonWriter;

/// The functions dictionary of `rrdcontext_to_json_v2()`: one entry per `"<version>|<name>"` in the order first seen,
/// with the attributes of the first host that has it and the `ni` of every host that does.
#[derive(Default)]
pub(super) struct Functions {
    entries: Vec<(Vec<u8>, Arc<Method>, Vec<usize>)>,
    by_key: HashMap<Vec<u8>, usize>,
}

impl Functions {
    /// A host's `nrpc_catalog_host_to_dict()` entries merged by C's insert and conflict callbacks.
    pub(super) fn add(&mut self, host: Vec<(Vec<u8>, Arc<Method>)>, ni: usize) {
        for (key, method) in host {
            match self.by_key.get(&key) {
                Some(&i) => self.entries[i].2.push(ni),
                None => {
                    self.by_key.insert(key.clone(), self.entries.len());
                    self.entries.push((key, method, vec![ni]));
                }
            }
        }
    }

    /// The `functions` array: each entry named after its key's first `|`; `mcp` leaves out `ni`, the priority (C's
    /// `int` printed as `uint64`) and the version.
    pub(super) fn to_json(&self, w: &mut JsonWriter, mcp: bool) {
        w.member_add_array(Some(b"functions"));
        for (key, method, ni) in &self.entries {
            let name = key.iter().position(|&b| b == b'|').map_or(&key[..], |i| &key[i + 1..]);
            w.add_array_item_object();
            w.member_add_string("name", name);
            w.member_add_string("help", &method.help);
            if !mcp {
                w.member_add_array(Some(b"ni"));
                for &n in ni {
                    w.add_array_item_uint64(n as u64);
                }
                w.array_close();
                w.member_add_uint64("priority", method.priority as u64);
                w.member_add_uint64("version", u64::from(method.version));
            }
            w.member_add_string("tags", &method.tags);
            w.member_add_array(Some(b"access"));
            for name in access::names(method.access) {
                w.add_array_item_string(name);
            }
            w.array_close();
            w.object_close();
        }
        w.array_close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netdata_agent_nrpc::catalog;
    use netdata_agent_nrpc::testing::inert;
    use netdata_agent_text::json::JsonOptions;

    /// C's functions dictionary (`api_v2_contexts.c:779-806`) and writer (`:1481-1516`): a name and version two hosts
    /// share keeps the first host's attributes and lists both; another version is its own entry; the bytes are C's
    /// (H9's oracle body).
    #[test]
    fn functions_merge_and_print_as_cs() {
        use netdata_agent_nrpc::{MethodDesc, Registry, Source};
        let desc = |name: &'static [u8], help: &'static [u8], version| MethodDesc {
            name,
            help,
            tags: b"",
            timeout_s: 10,
            priority: 0,
            version,
            access: access::ANONYMOUS_DATA,
            sync: false,
            source: Source::Stream,
            handler: inert(),
        };
        let (local, vnode) = (Registry::default(), Registry::default());
        local.register("l", &desc(b"difftest-same", b"same on localhost", 1)).unwrap();
        local.register("l", &desc(b"difftest-ver", b"version 1", 1)).unwrap();
        vnode.register("v", &desc(b"difftest-same", b"same on the vnode", 1)).unwrap();
        vnode.register("v", &desc(b"difftest-ver", b"version 2", 2)).unwrap();
        let mut functions = Functions::default();
        functions.add(catalog::to_dict(&local), 0);
        functions.add(catalog::to_dict(&vnode), 1);
        let printed = |mcp| {
            let mut w = JsonWriter::new(JsonOptions::MINIFY);
            functions.to_json(&mut w, mcp);
            w.finalize();
            String::from_utf8(w.into_bytes()).unwrap()
        };
        let entry = |name: &str, help: &str, ni: &str, version| {
            format!(
                concat!(
                    r#"{{"name":"{}","help":"{}","ni":[{}],"priority":100,"version":{},"#,
                    r#""tags":"top","access":["anonymous-data"]}}"#
                ),
                name, help, ni, version
            )
        };
        assert_eq!(
            printed(false),
            format!(
                r#"{{"functions":[{},{},{}]}}"#,
                entry("difftest-same", "same on localhost", "0,1", 1),
                entry("difftest-ver", "version 1", "0", 1),
                entry("difftest-ver", "version 2", "1", 2)
            )
        );
        assert!(printed(true).starts_with(concat!(
            r#"{"functions":[{"name":"difftest-same","help":"same on localhost","tags":"top","#,
            r#""access":["anonymous-data"]},"#
        )));
    }
}
