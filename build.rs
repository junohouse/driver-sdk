//! Bake the proxy contracts into the crate.
//!
//! `bundled()` used to read `$CARGO_MANIFEST_DIR/proxies` at runtime, which is a path on the
//! machine that *built* the binary. That is fine for `cargo run` and wrong everywhere else —
//! a container, a release artifact, or this crate consumed as a dependency, where the path is
//! somewhere in a cargo git checkout that may not survive.
//!
//! Reading them here instead means the contracts travel inside the binary, which is what
//! "bundled" was always supposed to mean.

use std::fmt::Write as _;

fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("proxies");
    println!("cargo:rerun-if-changed=proxies");

    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    // Sorted so the generated file is byte-stable between builds.
    files.sort();

    assert!(!files.is_empty(), "no proxy contracts in {}", dir.display());

    let mut out = String::from(
        "/// Every contract shipped with this crate, as (name, TOML source).\n\
         pub static PROXIES: &[(&str, &str)] = &[\n",
    );
    for path in &files {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        writeln!(
            out,
            "    ({stem:?}, include_str!({:?})),",
            path.to_string_lossy()
        )
        .expect("writing to a String cannot fail");
    }
    out.push_str("];\n");

    let dest = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("proxies.rs");
    std::fs::write(&dest, out).expect("writing the generated contract list");
}
