use std::env::var;
use tonic_build::{compile_protos, configure};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .out_dir("proto-source")
        .compile(&["proto/quote/streaming.proto"], &["proto/"])?;
    Ok(())
}
