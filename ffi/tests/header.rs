//! `include/freshen.h` is generated from the Rust source. This fails when the two
//! drift apart; regenerate with `FRESHEN_UPDATE_HEADER=1 cargo test -p freshen-ffi --test header`.

use std::{env, fs, path::PathBuf};

#[test]
fn header_matches_source() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = cbindgen::Config::from_file(dir.join("cbindgen.toml")).unwrap();
    let bindings = cbindgen::Builder::new()
        .with_crate(&dir)
        .with_config(config)
        .generate()
        .expect("cbindgen failed");
    let mut generated = Vec::new();
    bindings.write(&mut generated);
    let path = dir.join("include/freshen.h");
    if env::var_os("FRESHEN_UPDATE_HEADER").is_some() {
        fs::write(&path, &generated).unwrap();
        return;
    }
    let committed = fs::read(&path).unwrap_or_default();
    assert!(
        committed == generated,
        "include/freshen.h is stale; run: FRESHEN_UPDATE_HEADER=1 cargo test -p freshen-ffi --test header"
    );
}
