fn main() {
    tonic_prost_build::configure()
        .compile_protos(&["proto/slot_log.proto"], &["proto"])
        .unwrap();
}
