//! Generates the Rust message types from the shared schema in `proto/`.
//!
//! Every `.proto` file in `proto/link2linux/v1` is compiled, so a new
//! feature's schema file is picked up without touching this script.
//! `protoc` comes from the `PROTOC` environment variable or from `PATH`.

use std::fs;
use std::io;
use std::path::PathBuf;

/// Include root for imports, shared with the Android build.
const PROTO_ROOT: &str = "../../proto";
/// The package directory of the current major version.
const PROTO_PACKAGE_DIR: &str = "../../proto/link2linux/v1";

fn main() -> io::Result<()> {
    println!("cargo::rerun-if-changed={PROTO_PACKAGE_DIR}");
    println!("cargo::rerun-if-env-changed=PROTOC");

    let mut protos = fs::read_dir(PROTO_PACKAGE_DIR)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<PathBuf>>>()?;
    protos.retain(|path| path.extension().is_some_and(|ext| ext == "proto"));
    protos.sort();

    prost_build::Config::new().compile_protos(&protos, &[PROTO_ROOT])
}
