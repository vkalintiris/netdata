use std::io::Result;

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=../../streaming/pbser/proto/netdata/v1/netdata.proto");

    prost_build::compile_protos(
        &["netdata.proto"],
        &["../../streaming/pbser/proto/netdata/v1"],
    )?;
    Ok(())
}
