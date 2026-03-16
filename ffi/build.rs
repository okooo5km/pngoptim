// Copyright (c) 2026 okooo5km(十里). All rights reserved.
// Licensed under the MIT License.

fn main() {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let output_dir = std::path::Path::new(&crate_dir).join("generated");
    std::fs::create_dir_all(&output_dir).expect("Failed to create generated/ directory");

    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(cbindgen::Config::from_file("cbindgen.toml").unwrap())
        .generate()
        .expect("Unable to generate C bindings")
        .write_to_file(output_dir.join("pngoptim.h"));
}
