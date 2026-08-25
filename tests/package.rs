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
    let ir = manifest_with(
        "apple.tv.ir",
        "declarative",
        false,
        "variant_of = \"apple.tv\"",
    );
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

/// One table, and a field that means nothing for a kind is a build error rather than a
/// silence.
///
/// The silence is the bug worth preventing: a driver ships with a `port` on an IR emitter and
/// its author believes something dials it. Nothing ever did, and nothing ever said so.
#[test]
fn a_connection_is_checked_against_its_own_kind() {
    use driver_sdk::manifest::Manifest;
    let proxies = ProxyRegistry::bundled().unwrap();

    let dialled = Manifest::parse(&format!(
        "{}\n[[control]]\nid = 1\nkind = \"mqtt\"\nname = \"Broker\"\nport = 8883\ntls = true\n",
        manifest("a.net", "wasm", true)
    ))
    .unwrap();
    assert!(dialled.validate(&proxies).is_empty());

    let patched = Manifest::parse(&format!(
        "{}\n[[control]]\nid = 1\nkind = \"ir_out\"\nname = \"IR\"\nport = 4998\n",
        manifest("a.ir", "wasm", true)
    ))
    .unwrap();
    let errs = patched.validate(&proxies);
    assert!(
        errs.iter().any(|e| e.contains("`port` means nothing")),
        "a port on an emitter has to be refused: {errs:?}"
    );

    // Two of the same dialled kind: nothing downstream could say which one a device is on.
    let twice = Manifest::parse(&format!(
        "{}\n[[control]]\nid = 1\nkind = \"tcp\"\nname = \"A\"\n\n         [[control]]\nid = 2\nkind = \"tcp\"\nname = \"B\"\n",
        manifest("a.two", "wasm", true)
    ))
    .unwrap();
    assert!(
        twice
            .validate(&proxies)
            .iter()
            .any(|e| e.contains("two `Tcp`"))
    );

    // Two different kinds is the whole point of the table.
    let both = Manifest::parse(&format!(
        "{}\n[[control]]\nid = 1\nkind = \"tcp\"\nname = \"Security\"\nport = 12345\n\n         [[control]]\nid = 2\nkind = \"mqtt\"\nname = \"Automation\"\nport = 8883\ntls = true\n",
        manifest("a.panel", "wasm", true)
    ))
    .unwrap();
    assert!(
        both.validate(&proxies).is_empty(),
        "{:?}",
        both.validate(&proxies)
    );
}

/// A mesh node is matched on what its descriptor says, and a node that grew a cluster in a
/// firmware update still matches — a descriptor lists what a device can do, and requiring the
/// exact set would refuse the same product a year later.
#[test]
fn a_zigbee_fingerprint_matches_a_superset() {
    use driver_sdk::manifest::ZigbeeMatch;
    let rule = ZigbeeMatch {
        profile: 49297,
        endpoint: 1,
        in_clusters: vec![2, 3, 11, 13],
    };

    assert!(rule.matches(49297, 1, &[2, 3, 11, 13]));
    assert!(
        rule.matches(49297, 1, &[2, 3, 11, 13, 25]),
        "a superset still matches"
    );
    assert!(
        !rule.matches(49297, 1, &[2, 3, 11]),
        "a missing cluster does not"
    );
    assert!(
        !rule.matches(260, 1, &[2, 3, 11, 13]),
        "nor another profile"
    );
    assert!(
        !rule.matches(49297, 2, &[2, 3, 11, 13]),
        "nor another endpoint"
    );
}

/// The word a kind is written with, and the one every reader matches on.
///
/// `IrOut` debugs as `IrOut`; lowercasing that gives `irout`, which is not what any manifest
/// says and not what the catalog's own table of connection words is keyed on. It reached a
/// running controller before anybody looked.
#[test]
fn a_kind_serialises_as_the_word_the_manifest_uses() {
    use driver_sdk::manifest::ControlKind as K;
    for (kind, word) in [
        (K::IrOut, "ir_out"),
        (K::Serial, "serial"),
        (K::Relay, "relay"),
        (K::Contact, "contact"),
        (K::Network, "network"),
        (K::Tcp, "tcp"),
        (K::Mqtt, "mqtt"),
        (K::Hap, "hap"),
        (K::Zigbee, "zigbee"),
    ] {
        assert_eq!(kind.as_str(), word);
        // And it is the spelling a manifest actually parses, rather than a second list that
        // agrees with serde today.
        assert_eq!(
            serde_json::to_string(&kind).unwrap(),
            format!("\"{word}\""),
            "{kind:?} must serialise as it is written"
        );
    }
}

/// One port, two contracts, and a closed set of them.
///
/// The 3.5 mm jacks on the hardware are IR or serial depending on what somebody plugged in,
/// and nothing electrical decides it. What must not happen is a port claiming a contract the
/// hardware was never built for, because something downstream will route a room through it.
#[test]
fn a_combo_port_may_only_be_what_it_declared() {
    use driver_sdk::manifest::Manifest;
    let proxies = ProxyRegistry::bundled().unwrap();

    let m = Manifest::parse(
        "[driver]\nid = \"x.gc\"\nname = \"iTach\"\nversion = \"1.0.0\"\nruntime = \"wasm\"\n\n\
         [[proxy]]\nid = 1\ntype = \"ir_out\"\nalternates = [\"serial_port\"]\nname = \"Port 1\"\n\n\
         [[proxy]]\nid = 2\ntype = \"relay\"\nname = \"Relay 1\"\n",
    )
    .unwrap();
    assert!(
        m.validate(&proxies).is_empty(),
        "{:?}",
        m.validate(&proxies)
    );

    assert!(m.binding_may_be(1, "ir_out"), "what it already is");
    assert!(
        m.binding_may_be(1, "serial_port"),
        "and what else it can be"
    );
    assert!(
        !m.binding_may_be(1, "relay"),
        "but not a contract it never offered"
    );
    assert!(
        !m.binding_may_be(2, "ir_out"),
        "a plain port has no alternates"
    );
    assert!(
        !m.binding_may_be(9, "ir_out"),
        "nor does a proxy that does not exist"
    );

    // A typo'd alternate is a port that can never be switched, discovered by somebody trying.
    let typo = Manifest::parse(
        "[driver]\nid = \"x.y\"\nname = \"Y\"\nversion = \"1.0.0\"\nruntime = \"wasm\"\n\n\
         [[proxy]]\nid = 1\ntype = \"ir_out\"\nalternates = [\"serial\"]\n",
    )
    .unwrap();
    assert!(
        typo.validate(&proxies)
            .iter()
            .any(|e| e.contains("`serial` is not a proxy"))
    );

    // And naming what it already is says nothing, so it is a mistake rather than a no-op.
    let same = Manifest::parse(
        "[driver]\nid = \"x.z\"\nname = \"Z\"\nversion = \"1.0.0\"\nruntime = \"wasm\"\n\n\
         [[proxy]]\nid = 1\ntype = \"ir_out\"\nalternates = [\"ir_out\"]\n",
    )
    .unwrap();
    assert!(
        same.validate(&proxies)
            .iter()
            .any(|e| e.contains("already what it is"))
    );
}

/// A driver's screen can be a project, as long as what ships is one file.
///
/// The frame is `srcdoc`: the configurator hands it text, so there is no URL for a relative
/// `<script src>` to resolve against and nothing beside the page to fetch. That is a constraint
/// on the artifact and not on how it was written — a React app in a dozen files, built and
/// inlined, is the same one file. So the built copy wins, and a page that still loads a sibling
/// is refused here, where the bundler that should have inlined it can still be fixed. Left to a
/// house it is a pane that loads and does nothing, which looks like a pane with nothing to do.
#[test]
fn a_built_page_wins_and_a_page_needing_siblings_is_refused() {
    let dir = std::env::temp_dir().join(format!("juno-ui-pack-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ui").join("dist")).unwrap();
    std::fs::write(dir.join("manifest.toml"), manifest("test.ui", "declarative", true)).unwrap();
    std::fs::write(dir.join("commands.toml"), "command.on = { tx = \"ON\\r\" }").unwrap();

    // Written by hand and built: the build is what ships.
    std::fs::write(dir.join("ui").join("index.html"), "<p>by hand</p>").unwrap();
    std::fs::write(dir.join("ui").join("dist").join("index.html"), "<p>built</p>").unwrap();
    let out = dir.join("out");
    let built = Package::build(&dir, &out).expect("packs");
    assert_eq!(page_in(&built), "<p>built</p>");

    // Only written by hand: still fine. Most drivers will never have a build.
    std::fs::remove_file(dir.join("ui").join("dist").join("index.html")).unwrap();
    let built = Package::build(&dir, &out).expect("packs");
    assert_eq!(page_in(&built), "<p>by hand</p>");

    // A bundler that did not inline. This is the failure worth catching: it installs, and the
    // pane is blank.
    std::fs::write(dir.join("ui").join("index.html"),
                   "<div id=root></div><script src=\"/assets/main.js\"></script>").unwrap();
    let err = Package::build(&dir, &out).expect_err("refused");
    assert!(format!("{err}").contains("loads a file beside itself"), "{err}");

    // And a UI project with nothing built is somebody who forgot the build step.
    std::fs::remove_file(dir.join("ui").join("index.html")).unwrap();
    std::fs::write(dir.join("ui").join("package.json"), "{\"name\":\"ui\"}").unwrap();
    let err = Package::build(&dir, &out).expect_err("refused");
    assert!(format!("{err}").contains("build it before packing"), "{err}");
}

/// The page inside a built `.junodrv`.
fn page_in(archive: &std::path::Path) -> String {
    let file = std::fs::File::open(archive).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut page = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("ui/index.html").unwrap(), &mut page).unwrap();
    page
}
