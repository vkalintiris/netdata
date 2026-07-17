//! MANIFEST.json: the machine-readable index of every file in the bundle,
//! with its safe origin (command / source path / API endpoint), size, and
//! sanitization state. Emitted LAST so it indexes summary.txt and README.md.

use serde_json::json;

/// The closed set of capture kinds in the `netdata-support-bundle/v1`
/// schema. The strings are the on-wire contract; the enum keeps call sites
/// typo-proof and exhaustive.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum ManifestKind {
    Cmd,
    File,
    Api,
}

impl ManifestKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ManifestKind::Cmd => "cmd",
            ManifestKind::File => "file",
            ManifestKind::Api => "api",
        }
    }
}

pub struct ManifestRow {
    pub path: String,
    pub kind: ManifestKind,
    pub origin: String,
    pub title: String,
    pub bytes: u64,
    pub pii_obfuscated: bool,
}

#[derive(Default)]
pub struct Manifest {
    rows: Vec<ManifestRow>,
}

pub struct ManifestMeta {
    pub generated_utc: String,
    pub runtime_seconds: u64,
    pub pii_obfuscated: bool,
    pub agent_running: bool,
    pub agent_api_reachable: bool,
    pub is_container: bool,
}

impl Manifest {
    pub fn add(
        &mut self,
        path: &str,
        kind: ManifestKind,
        origin: &str,
        title: &str,
        bytes: u64,
        pii_obfuscated: bool,
    ) {
        // origin/title must stay single-line, the way the scripts kept them
        self.rows.push(ManifestRow {
            path: path.replace('\\', "/"),
            kind,
            origin: crate::util::flatten_single_line(origin, false),
            title: crate::util::flatten_single_line(title, false),
            bytes,
            pii_obfuscated,
        });
    }

    pub fn emit(&self, meta: &ManifestMeta) -> String {
        let files: Vec<serde_json::Value> = self
            .rows
            .iter()
            .map(|r| {
                json!({
                    "path": r.path,
                    "kind": r.kind.as_str(),
                    "origin": r.origin,
                    "title": r.title,
                    "bytes": r.bytes,
                    "pii_obfuscated": r.pii_obfuscated,
                })
            })
            .collect();
        let doc = json!({
            "schema": crate::consts::SCHEMA,
            "tool_version": crate::consts::TOOL_VERSION,
            "generated_utc": meta.generated_utc,
            "runtime_seconds": meta.runtime_seconds,
            "pii_obfuscated": meta.pii_obfuscated,
            "secrets_redacted": true,
            "agent_running": meta.agent_running,
            "agent_api_reachable": meta.agent_api_reachable,
            "is_container": meta.is_container,
            "files": files,
        });
        let mut s = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string());
        s.push('\n');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_matches_the_v1_schema_shape() {
        let mut m = Manifest::default();
        m.add(
            "01-system\\uname.txt",
            ManifestKind::Cmd,
            "uname -a",
            "Kernel\nand architecture",
            42,
            true,
        );
        let meta = ManifestMeta {
            generated_utc: "2026-07-18T00:00:00Z".to_string(),
            runtime_seconds: 3,
            pii_obfuscated: true,
            agent_running: false,
            agent_api_reachable: false,
            is_container: false,
        };
        let doc: serde_json::Value = serde_json::from_str(&m.emit(&meta)).unwrap();
        assert_eq!(doc["schema"], crate::consts::SCHEMA);
        assert_eq!(doc["tool_version"], crate::consts::TOOL_VERSION);
        assert_eq!(doc["secrets_redacted"], true);
        let files = doc["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        // backslashes normalize to forward slashes; titles stay single-line
        assert_eq!(files[0]["path"], "01-system/uname.txt");
        assert_eq!(files[0]["kind"], "cmd");
        assert_eq!(files[0]["title"], "Kernel and architecture");
        assert_eq!(files[0]["bytes"], 42);
    }
}
