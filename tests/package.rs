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
    manifest_with(id, runtime, primary, "")
}

/// The same, with extra lines inside `[driver]` — where they have to go, since everything
/// after the first `[[proxy]]` belongs to the proxy.
fn manifest_with(id: &str, runtime: &str, primary: bool, extra: &str) -> String {
    format!(
        r#"
[driver]
id = "{id}"
name = "Test {id}"
manufacturer = "Test"
version = "1.0.0"
runtime = "{runtime}"
primary = {primary}
{extra}
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
        (
            "manifests/thing.toml",
            manifest("thing", "native", true).as_bytes(),
        ),
        (
            "manifests/thing.ir.toml",
            manifest("thing.ir", "declarative", false).as_bytes(),
        ),
        ("driver-macos-aarch64.dylib", b"NATIVE"),
        ("driver-linux-x86_64.so", b"NATIVE"),
        ("driver-linux-aarch64.so", b"NATIVE"),
        ("driver-windows-x86_64.dll", b"NATIVE"),
        (
            "commands.toml",
            b"command.menu = { control = 1, invoke = \"send\" }",
        ),
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
        (
            "manifests/thing.toml",
            manifest("thing", "native", true).as_bytes(),
        ),
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

/// A variant that names a driver the package does not carry is refused whole.
///
/// The failure it prevents is silent: the catalog folds a variant into the product it names, so
/// pointing at a driver that is not there hides the row behind something that never arrives.
/// The hardware becomes unreachable and nothing anywhere says why.
#[test]
fn a_variant_must_name_a_sibling() {
    let proxies = ProxyRegistry::bundled().unwrap();

    let lead = manifest("apple.tv", "declarative", true);
    let ir = manifest_with("apple.tv.ir", "declarative", false, "variant_of = \"apple.tv\"");
    let good = archive_of(&[
        ("manifests/tv.toml", lead.as_bytes()),
        ("manifests/ir.toml", ir.as_bytes()),
        ("commands.toml", b"[commands]\n"),
    ]);
    Package::read(std::io::Cursor::new(good), &proxies).expect("a sibling variant is fine");

    // The same package with the variant pointing somewhere else entirely.
    let stray = manifest_with(
        "apple.tv.ir",
        "declarative",
        false,
        "variant_of = \"roku.player\"",
    );
    let bad = archive_of(&[
        ("manifests/tv.toml", lead.as_bytes()),
        ("manifests/ir.toml", stray.as_bytes()),
        ("commands.toml", b"[commands]\n"),
    ]);
    let err = Package::read(std::io::Cursor::new(bad), &proxies)
        .expect_err("a variant of something not in the package is refused")
        .to_string();
    assert!(err.contains("roku.player"), "{err}");
}

/// What a product is, and what it is called, both fall back rather than coming out empty.
#[test]
fn kind_and_product_fall_back_to_the_driver() {
    use driver_sdk::manifest::Manifest;

    // Nothing declared: the proxy it leads with, and its own name.
    let plain = Manifest::parse(&manifest("vizio.tv", "wasm", true)).unwrap();
    assert_eq!(plain.kind(), Some("media_player"));
    assert_eq!(plain.product(), "Test vizio.tv");

    // A hub says both, because neither can be read off a bridge.
    let hub = Manifest::parse(&manifest_with(
        "signify.hue.bridge",
        "wasm",
        true,
        "product = \"Philips Hue\"\nkind = \"light\"",
    ))
    .unwrap();
    assert_eq!(hub.kind(), Some("light"));
    assert_eq!(hub.product(), "Philips Hue");

    // And a kind that is not a proxy is a build error, not a group nobody can find.
    let proxies = ProxyRegistry::bundled().unwrap();
    let typo = Manifest::parse(&manifest_with("x.y", "wasm", true, "kind = \"lights\"")).unwrap();
    assert!(
        typo.validate(&proxies).iter().any(|e| e.contains("lights")),
        "a typo'd kind must fail the manifest"
    );
}

/// What tells two variants apart, and therefore whether anybody has to be asked.
///
/// The distinction is the whole reason the Items panel asks about an Apple TV and not about a
/// Roku. Reached differently — a Companion socket or an IR emitter — and only somebody
/// standing in the room knows which; reached the same way, and the difference is what the box
/// *is*, which its own setup flow can read off the device.
#[test]
fn reach_is_the_control_then_the_transport() {
    use driver_sdk::manifest::Manifest;

    let network = Manifest::parse(&format!(
        "{}\n[[transport]]\nkind = \"network\"\n",
        manifest("a.net", "wasm", true)
    ))
    .unwrap();
    assert_eq!(network.reach(), vec!["network".to_string()]);

    // A control outranks a transport rather than joining it: what an installer wired is the
    // answer somebody needs, and a driver that declares both is reached through the wire.
    let emitter = Manifest::parse(&format!(
        "{}\n[[transport]]\nkind = \"network\"\n\n[[control]]\nid = 1\nkind = \"ir_out\"\nname = \"IR\"\n",
        manifest("a.ir", "wasm", true)
    ))
    .unwrap();
    assert_eq!(emitter.reach(), vec!["ir_out".to_string()]);

    // Neither: a child of a bridge, reached through whatever its parent holds.
    let child = Manifest::parse(&manifest("a.child", "wasm", false)).unwrap();
    assert!(child.reach().is_empty());
}
