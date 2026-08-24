//! Validate and package a driver, without a controller.
//!
//! `junod pack` does this too, but `junod` lives in the controller's repository, which is
//! private — so using it meant every driver's CI needed read access to a repository the driver
//! author has nothing to do with. Everything the packaging step actually needs is the
//! contracts and the manifest format, and both are in this crate.
//!
//!     junodrv pack <dir> [--out <dir>]   build a .junodrv, refusing one that would not install
//!     junodrv check <dir>                validate only
//!     junodrv entry <pkg.junodrv> …      emit the registry index rows for a built package
//!     junodrv docs [--out <dir>]         render the contracts as the published proxy reference
//!
//! Deliberately not `clap`. A handful of subcommands and one flag do not justify a dependency
//! on a crate this size, and every driver author would pay for it in build time.

use driver_sdk::catalog::{DiscoveryHints, Entry, Release, Source};
use driver_sdk::package::Package;
use driver_sdk::proxy::{ProxyRegistry, docgen};
use std::path::{Path, PathBuf};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

const USAGE: &str = "\
usage:
  junodrv pack <dir> [--out <dir>]   validate and build a .junodrv
  junodrv check <dir>                validate only
  junodrv entry <pkg.junodrv> --repo R --url U --sha256 S [--version V] [--description D]
                              [--source open|closed]
                                     emit the registry index rows for a built package
  junodrv docs [--out <dir>]         render the proxy reference as markdown

  <dir> holds manifest.toml, or manifests/*.toml for a package with several drivers.
  --out defaults to ./dist, or ../docs/content/proxies for `docs`";

fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        anyhow::bail!("{USAGE}");
    };
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    // Every command but `docs` is named after something on disk; `docs` renders what is
    // compiled in and has nothing to point at.
    let dir = || {
        args.get(1)
            .filter(|a| !a.starts_with("--"))
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("{USAGE}"))
    };
    let out = |default: &str| flag("--out").map_or_else(|| PathBuf::from(default), PathBuf::from);

    // The contracts are compiled in, so there is nothing to fetch and nothing to point at.
    let registry = ProxyRegistry::bundled()?;

    match cmd {
        "check" => {
            let manifests = read(&dir()?, &registry)?;
            for m in &manifests {
                println!("{} v{}  ok", m.0, m.1);
            }
            Ok(())
        }
        "pack" => {
            let (dir, out) = (dir()?, out("dist"));
            // Validate first and separately, so a package that would not install fails here
            // rather than producing an archive somebody then tries to ship.
            let manifests = read(&dir, &registry)?;
            std::fs::create_dir_all(&out)?;
            let built = Package::build(&dir, &out)?;
            let lead = manifests.first().expect("at least one manifest");
            println!("{} v{} -> {}", lead.0, lead.1, built.display());
            Ok(())
        }
        // The published reference is generated from the contracts, so it cannot describe a
        // command a driver would not be allowed to declare. The default output path assumes
        // the docs repo is checked out beside this one, which is what CI does.
        "docs" => {
            let out = out("../docs/content/proxies");
            std::fs::create_dir_all(&out)?;
            let files = docgen::all(&registry);
            for (name, body) in &files {
                std::fs::write(out.join(name), body)
                    .map_err(|e| anyhow::anyhow!("writing {name}: {e}"))?;
            }
            println!("wrote {} files to {}", files.len(), out.display());
            Ok(())
        }
        // Describing an artifact you just built is part of publishing it, so it belongs
        // wherever the packaging does. Opening the archive rather than reading the source tree
        // validates as a side effect: a row can only be emitted for a package that installs.
        "entry" => {
            let dir = dir()?;
            let pkg = Package::open(&dir, &registry)?;
            let size = std::fs::metadata(&dir).map(|m| m.len()).unwrap_or(0);
            let version = flag("--version").unwrap_or_else(|| pkg.manifest.driver.version.clone());

            // Unstated rather than guessed. An absent `source` reads as open in the catalog,
            // which is right for every row written before the field existed, and a wrong guess
            // here would either 404 a Source link or hide one that works.
            let source = match flag("--source").as_deref() {
                None => None,
                Some("open") => Some(Source::Open),
                Some("closed") => Some(Source::Closed),
                Some(other) => anyhow::bail!("--source is open or closed, not `{other}`"),
            };

            let entries: Vec<Entry> = std::iter::once(&pkg.manifest)
                .chain(pkg.extra.iter())
                .map(|m| Entry {
                    id: m.driver.id.clone(),
                    name: m.driver.name.clone(),
                    manufacturer: m.driver.manufacturer.clone(),
                    parent: m.driver.parent.clone(),
                    product: m.driver.product.clone(),
                    // Resolved rather than copied: a driver that declared no `kind` still has
                    // to land in a group, and working the fallback out here means the catalog
                    // at the far end never has to guess from `proxies` — which gets every hub
                    // wrong, and would get it wrong differently in each reader.
                    kind: m.kind().map(str::to_string),
                    variant_of: m.driver.variant_of.clone(),
                    reach: m.reach(),
                    repo: flag("--repo").unwrap_or_default(),
                    source,
                    proxies: m.proxy.iter().map(|p| p.ty.clone()).collect(),
                    runtime: format!("{:?}", m.driver.runtime).to_lowercase(),
                    description: flag("--description").unwrap_or_default(),
                    discovery: DiscoveryHints {
                        mdns: m.discovery.mdns.clone(),
                        ssdp: m.discovery.ssdp.clone(),
                        sddp: m.discovery.sddp.clone(),
                        mac_oui: m.discovery.mac_oui.clone(),
                        udp: m.discovery.udp.clone(),
                        adopt_as: m.discovery.adopt_as.clone(),
                    },
                    versions: vec![Release {
                        // Every driver in a package ships at the package's version. Letting
                        // them differ would mean one artifact claiming two versions of itself.
                        version: version.clone(),
                        core_req: m.driver.min_core.clone().unwrap_or_default(),
                        url: flag("--url").unwrap_or_default(),
                        sha256: flag("--sha256").unwrap_or_default(),
                        // What build this is, while nothing is released — see `Release::commit`.
                        commit: flag("--commit").unwrap_or_default(),
                        size,
                    }],
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&entries)?);
            Ok(())
        }
        other => anyhow::bail!("no command `{other}`\n\n{USAGE}"),
    }
}

/// Parse every manifest in `dir` and check it against the real contracts.
fn read(dir: &Path, registry: &ProxyRegistry) -> anyhow::Result<Vec<(String, String)>> {
    let mut sources = Vec::new();
    let root = dir.join("manifest.toml");
    if root.is_file() {
        sources.push(std::fs::read_to_string(&root)?);
    }
    let extra = dir.join("manifests");
    if extra.is_dir() {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&extra)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        paths.sort();
        for p in paths {
            sources.push(std::fs::read_to_string(&p)?);
        }
    }
    if sources.is_empty() {
        anyhow::bail!(
            "{} declares no drivers: expected manifest.toml or manifests/*.toml",
            dir.display()
        );
    }

    let mut out = Vec::new();
    for src in &sources {
        let manifest: driver_sdk::manifest::Manifest = toml::from_str(src)?;
        let errs = manifest.validate(registry);
        if !errs.is_empty() {
            anyhow::bail!("{}:\n  {}", manifest.driver.id, errs.join("\n  "));
        }
        out.push((manifest.driver.id.clone(), manifest.driver.version.clone()));
    }
    Ok(out)
}
