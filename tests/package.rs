//! Packaging rules that are not obvious from the struct.
//!
//! Needs `--features pack`, which is what `[[test]] required-features` in `Cargo.toml` arranges;
//! without it `driver_sdk::package` does not exist and this file would not compile.

use driver_sdk::package::Package;
use driver_sdk::proxy::ProxyRegistry;
use std::io::Write;

/// A `.junodrv` built by hand, so a test can make one that is wrong in exactly one way.
fn archive_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut out);
    for (name, bytes) in files {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    out.into_inner()
}

fn manifest(id: &str, runtime: &str, primary: bool) -> String {
    format!(
        r#"
[driver]
id = "{id}"
name = "Test {id}"
manufacturer = "Test"
version = "1.0.0"
runtime = "{runtime}"
primary = {primary}

[[proxy]]
id = 1
type = "media_player"
primary = true
name = "Player"
capabilities = {{ has_transport = true }}
"#
    )
}

/// A package may carry two runtimes, and each driver gets the file *its own* manifest asks for.
///
/// The case is one product reachable two ways — an Apple TV over its Companion link, or the same
/// television over an IR emitter, which is a `commands.toml` and no code at all. Before this, the
/// lead manifest's runtime decided for the whole package: a native lead meant the declarative
/// sibling was handed a dylib, and the only way to ship both was two catalog entries.
///
/// The lead's own payload still lands in `payload`/`payload_name`; every driver's is in
/// `payloads`. That is what lets a controller skip a runtime nobody adopted.
#[test]
fn each_driver_gets_the_payload_its_own_runtime_asks_for() {
    let proxies = ProxyRegistry::bundled().unwrap();
    let bytes = archive_of(&[
        ("manifests/thing.toml", manifest("thing", "native", true).as_bytes()),
        (
            "manifests/thing.ir.toml",
            manifest("thing.ir", "declarative", false).as_bytes(),
        ),
        ("driver-macos-aarch64.dylib", b"NATIVE"),
        ("driver-linux-x86_64.so", b"NATIVE"),
        ("driver-linux-aarch64.so", b"NATIVE"),
        ("driver-windows-x86_64.dll", b"NATIVE"),
        ("commands.toml", b"command.menu = { control = 1, invoke = \"send\" }"),
    ]);

    let pkg = Package::read(std::io::Cursor::new(bytes), &proxies).unwrap();

    assert_eq!(pkg.manifest.driver.id, "thing", "`primary` names the lead");
    assert_eq!(
        pkg.payloads["thing"].1, b"NATIVE",
        "the native driver gets the plugin for this machine"
    );
    assert!(
        pkg.payloads["thing.ir"].0.ends_with("commands.toml"),
        "the declarative driver gets commands.toml, not the dylib: {}",
        pkg.payloads["thing.ir"].0
    );
    assert_eq!(
        pkg.payload, pkg.payloads["thing"].1,
        "`payload` stays the lead's, so nothing that reads it changes meaning"
    );
}

/// The per-manifest check is a check, not a lookup — a declarative driver that ships no
/// `commands.toml` is refused rather than loaded with whatever else was in the archive.
///
/// This is the rule the old code enforced once for the package, against the lead's runtime only.
/// Running it per manifest is the reason it now catches a sibling.
#[test]
fn a_sibling_missing_its_payload_is_refused() {
    let proxies = ProxyRegistry::bundled().unwrap();
    let bytes = archive_of(&[
        ("manifests/thing.toml", manifest("thing", "native", true).as_bytes()),
        (
            "manifests/thing.ir.toml",
            manifest("thing.ir", "declarative", false).as_bytes(),
        ),
        ("driver-macos-aarch64.dylib", b"NATIVE"),
        ("driver-linux-x86_64.so", b"NATIVE"),
        ("driver-linux-aarch64.so", b"NATIVE"),
        ("driver-windows-x86_64.dll", b"NATIVE"),
        // no commands.toml
    ]);

    let err = Package::read(std::io::Cursor::new(bytes), &proxies)
        .expect_err("a declarative driver with no commands.toml must not load");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("thing.ir") && msg.contains("commands.toml"),
        "the message must name the driver and the missing file: {msg}"
    );
}
