use std::collections::HashMap;
use std::path::Path;
use tempfile::TempDir;

const TONIC_OUT_DIR: &str = "src/proto/tonic";
const PROTO_FILES: &[&str] = &[
    "proto/netdata/protocol/v1/functions.proto",
    "proto/netdata/protocol/v1/agent/netdata_service.proto",
    "proto/netdata/protocol/v1/plugin/plugin_service.proto",
];
const PROTO_INCLUDES: &[&str] = &["proto"];

#[cfg(feature = "gen-tonic")]
#[test]
fn build_tonic() {
    let before_build = build_content_map(TONIC_OUT_DIR);

    let out_dir = TempDir::new().expect("failed to create temp dir to store the generated files");

    // Build the generated files with full tonic gRPC support
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .server_mod_attribute(".", "#[cfg(feature = \"gen-tonic\")]")
        .client_mod_attribute(".", "#[cfg(feature = \"gen-tonic\")]")
        .type_attribute(
            ".",
            "#[cfg_attr(feature = \"with-serde\", derive(serde::Serialize, serde::Deserialize))]",
        )
        .type_attribute(
            ".",
            "#[cfg_attr(feature = \"with-serde\", serde(rename_all = \"camelCase\"))]",
        )
        .out_dir(out_dir.path())
        .compile_protos(PROTO_FILES, PROTO_INCLUDES)
        .expect("cannot compile protobuf using tonic-build");

    let after_build = build_content_map(out_dir.path());
    ensure_files_are_same(before_build, after_build, TONIC_OUT_DIR);
}

fn build_content_map(path: impl AsRef<Path>) -> HashMap<String, String> {
    std::fs::read_dir(path)
        .expect("cannot open directory of generated files")
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("rs"))
        .map(|entry| {
            let path = entry.path();
            let file_name = path
                .file_name()
                .expect("file name should always exist for generated files");

            let file_contents = std::fs::read_to_string(path.clone())
                .expect("cannot read from existing generated file");

            (file_name.to_string_lossy().to_string(), file_contents)
        })
        .collect()
}

fn ensure_files_are_same(
    before_build: HashMap<String, String>,
    after_build: HashMap<String, String>,
    target_dir: &'static str,
) {
    if after_build == before_build {
        return;
    }

    if std::env::var("CI").is_ok() {
        panic!("generated file has changed but it's a CI environment, please rerun this test locally and commit the changes");
    }

    // if there is at least one changes we will just copy the whole directory over
    for (file_name, content) in after_build {
        std::fs::write(Path::new(target_dir).join(file_name), content)
            .expect("cannot write to the proto generate file. If it's happening in CI env, please rerun the test locally and commit the change");
    }

    panic!("generated file has changed, please commit the change file and rerun the test");
}
