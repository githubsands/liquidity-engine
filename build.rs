use std::{env, path::PathBuf};

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!("out directory is {}", out_dir.to_str().unwrap());
    tonic_build::configure()
        .build_server(true)
        .out_dir("proto-source/src")
        .file_descriptor_set_path(out_dir.join("quote.bin"))
        .compile(&["proto/streaminge.proto"], &["proto"])
        .unwrap();
}
