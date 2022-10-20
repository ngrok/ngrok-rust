use std::path::Path;

use prost_build::Config;

fn main() -> Result<(), &'static str> {
    let proto_path: &Path = "./src/tunnel.proto".as_ref();

    // directory the main .proto file resides in
    let proto_dir = proto_path
        .parent()
        .expect("proto file should reside in a directory");

    let mut builder = Config::new();

    if let Err(e) = builder.compile_protos(&[proto_path], &[proto_dir]) {
        println!("{}", e);
        return Err("prost_build failed");
    }
    Ok(())
}
