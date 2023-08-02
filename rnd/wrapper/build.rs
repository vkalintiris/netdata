use std::env;
use std::path::PathBuf;

fn main() {
    // Tell cargo to look for shared libraries in the specified directory
    // println!("cargo:rustc-link-search=/path/to/lib");

    // Tell cargo to tell rustc to link the system bzip2
    // shared library.
    // println!("cargo:rustc-link-lib=bz2");

    // Tell cargo to invalidate the built crate whenever the wrapper changes
    // println!("cargo:rerun-if-changed=wrapper.h");

    // The bindgen::Builder is the main entry point
    // to bindgen, and lets you build up options for
    // the resulting bindings.
    let bindings = bindgen::Builder::default()
        // The input header we would like to generate
        // bindings for.
        .clang_arg("-I../../")
        .clang_arg("-I/opt/homebrew/include")
        .clang_arg("-I../../web/server/h2o/libh2o/include")
        .clang_arg("-DHAVE_CONFIG_H")
        .clang_arg("-DJU_LITTLE_ENDIAN")
        .clang_arg("-DJU_64BIT")
        .clang_arg("-DJUDYL")
        .clang_arg("-DJUDYL")
        .clang_arg("-I/Users/vk/repos/nd/refse/libnetdata/libjudy/src")
        .clang_arg("-I/Users/vk/repos/nd/refse/libnetdata/libjudy/src/JudyCommon")
        .header("../../daemon/common.h")
        .clang_arg("-I../../mqtt_websockets/src/include")
        .clang_arg("-I../../mqtt_websockets/c-rbuf/include")
        .clang_arg("-I../../aclk/aclk-schemas")
        .clang_arg("-DDLIB_NO_GUI_SUPPORT")
        .clang_arg("-I../../ml/dlib")
        .clang_arg("-I/opt/homebrew/opt/openssl@3/include")
        // Tell cargo to invalidate the built crate whenever any of the
        // included header files changed.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        // Finish the builder and generate the bindings.
        .generate()
        // Unwrap the Result and panic on failure.
        .expect("Unable to generate bindings");

    println!("cargo:rustc-link-search=native=/Users/vk/repos/nd/refse");
    println!("cargo:rustc-link-lib=static=nd");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}
