//! `.junodrv` packages — a driver as one file you can hand someone.
//!
//! ```text
//! manifest.toml      one driver — identity, proxies, capabilities, properties, discovery
//! manifests/*.toml   several drivers, all of them, sharing the one payload
//! driver-<os>-<arch> } the payload, matching the manifest's runtime. A native package
//!   .dylib .so .dll   } carries one file per platform it was built for, and the controller
//! driver.wasm         } takes the one naming its own — architecture included, because an
//! commands.toml       } x86-64 and an aarch64 `.so` are not interchangeable
//! docs/README.md     shown in the driver pane
//! icons/             128 / 256 / 512 png
//! ```
//!
//! A package declaring several drivers puts *all* of them in `manifests/` rather than leading
//! with one at the root and hiding the rest in a directory — there is one place to look, and
//! no file is privileged by where it sits. Which one leads is then said outright, by `primary`
//! or by a `parent` relationship: see [`lead_index`].
//!
//! It is a zip. Core opens it, validates every manifest against the proxy contracts *before*
//! loading any code, and only then hands the payload to a runtime.

use crate::manifest::{Manifest, Runtime as RuntimeKind};
use crate::proxy::ProxyRegistry;
use anyhow::{Context, Result, bail};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Which of a package's manifests names the package.
///
/// Only comes up when the drivers all live in `manifests/`, since a lone `manifest.toml`
/// leads by definition. In order:
///
/// 1. Whichever says `primary = true`. Siblings — a Roku TV and a Roku player — have no other
///    way to express which is the headline, so the package says.
/// 2. A root device, over the children that name it as their `parent`. A Hue bridge leads its
///    bulbs without anyone having to write it down, because the relationship already says so.
/// 3. The first that is nobody's child, then simply the first. Arbitrary, but stable across
///    builds, which is what a filename needs.
fn lead_index(manifests: &[Manifest]) -> usize {
    manifests
        .iter()
        .position(|m| m.driver.primary)
        .or_else(|| {
            manifests.iter().position(|m| {
                manifests
                    .iter()
                    .any(|c| c.driver.parent.as_deref() == Some(m.driver.id.as_str()))
            })
        })
        .or_else(|| manifests.iter().position(|m| m.driver.parent.is_none()))
        .unwrap_or(0)
}

/// What this machine's plugin is called inside a package: `driver-linux-aarch64.so`.
///
/// Architecture is in the name, not just the extension. It used to be extension alone, which
/// cannot tell an x86-64 `.so` from an aarch64 one — so a package built on an Intel runner was
/// handed to an ARM controller, which failed at `dlopen` with a message about a missing
/// symbol. A controller in a house on a small ARM box is the normal case, not the exotic one.
pub fn platform_payload() -> String {
    let ext = if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    };
    format!(
        "driver-{}-{}.{ext}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Whether a compiled library in a shared `target/` belongs to the driver in `dir`.
///
/// The library's name comes from the package's own `Cargo.toml`, not from the directory it
/// sits in. Those used to be the same word and are not any more: a driver's repo is named for
/// the product (`signify-hue`, `lutron-caseta/leap`) while its crate is named for what it
/// exports (`juno_driver_hue`). Guessing from the directory silently found nothing.
pub fn belongs_to(lib: &Path, dir: &Path) -> bool {
    let Some(stem) = lib.file_stem().map(|s| s.to_string_lossy().to_string()) else {
        return false;
    };
    let stem = stem.trim_start_matches("lib");

    if let Some(name) = lib_name(dir) {
        return stem == name;
    }

    // No Cargo.toml: a declarative driver, which has no compiled library to match anyway.
    let Some(name) = dir
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
    else {
        return true;
    };
    let wanted = name.replace('-', "_");
    stem.ends_with(&wanted) || stem == format!("juno_driver_{wanted}")
}

/// The library name cargo will produce for the package in `dir`: `[lib] name` if it sets one,
/// otherwise the package name with dashes turned into underscores.
pub fn lib_name(dir: &Path) -> Option<String> {
    let src = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let toml: toml::Value = src.parse().ok()?;
    if let Some(name) = toml
        .get("lib")
        .and_then(|l| l.get("name"))
        .and_then(toml::Value::as_str)
    {
        return Some(name.to_string());
    }
    Some(
        toml.get("package")?
            .get("name")?
            .as_str()?
            .replace('-', "_"),
    )
}

/// Every compiled payload in a package, whatever platform it was built for.
pub fn is_payload(name: &str) -> bool {
    name.starts_with("driver-")
        && (name.ends_with(".dylib") || name.ends_with(".so") || name.ends_with(".dll"))
}

/// A package that has been opened and checked, but whose code has not run yet.
///
/// A package may declare more than one driver: `manifest.toml` for the one it leads with,
/// plus `manifests/*.toml`. A Hue package ships the bridge and the bulb together because
/// neither is useful alone, and shipping them apart means a version skew nobody can see.
#[derive(Debug, Clone)]
pub struct Package {
    pub manifest: Manifest,
    /// Additional drivers in the same package, sharing the one payload.
    pub extra: Vec<Manifest>,
    /// The payload file's name inside the archive, and its bytes.
    pub payload_name: String,
    pub payload: Vec<u8>,
    pub readme: Option<String>,
    /// Where the archive came from, when it was a file.
    pub source: Option<PathBuf>,
}

impl Package {
    /// Read and validate a `.junodrv`.
    ///
    /// Validation happens before anything is loaded, so a package with a typo'd capability is
    /// rejected without its code ever running.
    pub fn open(path: &Path, proxies: &ProxyRegistry) -> Result<Package> {
        let file =
            std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let mut pkg =
            Package::read(file, proxies).with_context(|| format!("reading {}", path.display()))?;
        pkg.source = Some(path.to_path_buf());
        Ok(pkg)
    }

    pub fn read<R: Read + std::io::Seek>(reader: R, proxies: &ProxyRegistry) -> Result<Package> {
        let mut zip = zip::ZipArchive::new(reader).context("not a valid .junodrv archive")?;

        // A package with one driver keeps it in `manifest.toml`; a package with several puts
        // them all in `manifests/`. Both are read the same way — collect every manifest, then
        // work out which one leads.
        let mut names: Vec<String> = (0..zip.len())
            .filter_map(|i| {
                let name = zip.by_index(i).ok()?.name().to_string();
                (name.starts_with("manifests/") && name.ends_with(".toml")).then_some(name)
            })
            .collect();
        names.sort();
        let has_root = zip.by_name("manifest.toml").is_ok();
        if has_root {
            names.insert(0, "manifest.toml".to_string());
        }
        if names.is_empty() {
            bail!("the archive declares no drivers: no manifest.toml and no manifests/*.toml");
        }

        let mut manifests = Vec::new();
        for name in &names {
            let mut src = String::new();
            zip.by_name(name)?.read_to_string(&mut src)?;
            let m = Manifest::parse(&src).with_context(|| format!("{name} is malformed"))?;
            let errs = m.validate(proxies);
            if !errs.is_empty() {
                bail!(
                    "{} is not a valid driver:\n  {}",
                    m.driver.id,
                    errs.join("\n  ")
                );
            }
            manifests.push(m);
        }

        let lead = if has_root { 0 } else { lead_index(&manifests) };
        let manifest = manifests.remove(lead);
        let extra = manifests;

        // The payload has to match what the manifest claims, or a `declarative` driver could
        // ship a binary and get it loaded.
        let wanted: &[&str] = match manifest.driver.runtime {
            RuntimeKind::Declarative => &["commands.toml"],
            RuntimeKind::Wasm => &["driver.wasm"],
            RuntimeKind::Python => &["driver.py"],
            // Named per platform inside the archive, and resolved below — one package
            // carries every platform it was built for.
            RuntimeKind::Native => &[],
            // Nothing to carry, and a package claiming otherwise is asking for code that does
            // not exist. `builtin` means core registers the driver itself — the virtual devices
            // behind `run --demo` — so an archive declaring one is either a mistake or an
            // attempt to have a payload loaded under a name that skips the payload checks.
            RuntimeKind::Builtin => {
                bail!(
                    "{} declares `runtime = \"builtin\"`, which is core's own — a package \
                     cannot carry one",
                    manifest.driver.id
                );
            }
            // Nothing. Not "resolved below" like a native plugin, but genuinely nothing: an
            // adapter has no in-process code, and the thing that runs is the `exec` in its
            // `[adapter]` table against the package's own tree. Demanding a payload here is how
            // an adapter fails to package with a message about a missing file.
            RuntimeKind::Adapter => &[],
        };

        let mut payload_name = String::new();
        let mut payload = Vec::new();
        for name in wanted {
            if let Ok(mut f) = zip.by_name(name) {
                payload_name = (*name).to_string();
                f.read_to_end(&mut payload)?;
                break;
            }
        }
        // A native plugin is platform-specific. A package may carry several — one per
        // platform — so take the one this machine can actually load rather than the first
        // one in the archive, which is how a macOS build ends up being handed to Linux.
        if payload.is_empty() && manifest.driver.runtime == RuntimeKind::Native {
            let ours = platform_payload();
            let names: Vec<String> = (0..zip.len())
                .filter_map(|i| {
                    let n = zip.by_index(i).ok()?.name().to_string();
                    is_payload(&n).then_some(n)
                })
                .collect();

            match names.iter().find(|n| **n == ours) {
                Some(n) => {
                    let n = n.clone();
                    zip.by_name(&n)?.read_to_end(&mut payload)?;
                    payload_name = n;
                }
                None => {
                    if !names.is_empty() {
                        // Naming what it *does* have is the whole point: "no build for your
                        // platform" sends someone looking at their controller, when the answer
                        // is that the driver's CI does not build for it yet.
                        bail!(
                            "{} has no build for this machine: it needs `{ours}` and the \
                             package carries {}",
                            manifest.driver.id,
                            names.join(", ")
                        );
                    }
                }
            }
        }
        // An adapter is the one runtime with nothing to load here, so an empty payload is
        // correct rather than missing. Everything else that reaches this point asked for a file
        // and did not get it.
        if payload.is_empty() && manifest.driver.runtime != RuntimeKind::Adapter {
            bail!(
                "{} declares runtime `{:?}` but the archive has no {}",
                manifest.driver.id,
                manifest.driver.runtime,
                wanted.join(" or ")
            );
        }

        let mut readme = String::new();
        if let Ok(mut f) = zip.by_name("docs/README.md") {
            let _ = f.read_to_string(&mut readme);
        }

        Ok(Package {
            manifest,
            extra,
            payload_name,
            payload,
            readme: (!readme.is_empty()).then_some(readme),
            source: None,
        })
    }

    /// Extract the whole archive into `into`, replacing whatever was there.
    ///
    /// Only adapters need this. Every other runtime is a single file core reads into memory,
    /// but an adapter's package *is* the tree its process runs — a binary, a script, whatever
    /// it ships beside them — so it has to exist on a disk the child can be pointed at.
    ///
    /// Replaced rather than merged: an upgrade that leaves a deleted file behind is a process
    /// running a mix of two versions, which is the kind of bug nobody reproduces.
    pub fn unpack(path: &Path, into: &Path) -> Result<()> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        let mut zip = zip::ZipArchive::new(file)?;

        // Extract beside the destination and move it into place, rather than clearing the
        // destination and filling it in.
        //
        // The old order left a window: between the `remove_dir_all` and the last file written,
        // the tree on disk was partial. Two unpacks of the same package racing — a parallel
        // test run, two controllers on one machine, an install while a supervisor is reading
        // the tree — meant one deleting what the other was still writing, which surfaces as
        // `Directory not empty` or a half-populated adapter that exec's a file that is not
        // there yet. An adapter package is thousands of small files, so the window is wide.
        //
        // A rename is atomic within a filesystem, so the destination is either the old tree or
        // the whole new one, never half of either. The staging name carries the pid so two
        // processes do not collide in the staging step either.
        // Unique per unpack, not per process: two threads in one controller install the same
        // package at the same time as readily as two processes do, and a pid alone would have
        // them staging into the same directory.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let ticket = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let staging = into.with_file_name(format!(
            "{}.unpack-{}-{}",
            into.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id(),
            ticket
        ));
        let _ = std::fs::remove_dir_all(&staging);
        if let Some(parent) = into.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(&staging)?;
        let into = &staging;

        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            // `enclosed_name` is the whole defence against an archive containing `../../etc`.
            // A sideloaded `.junodrv` is a file somebody downloaded, so this is a trust
            // boundary and the entry is skipped rather than sanitised — a package that needs
            // to escape its own directory is not one to guess the intentions of.
            let Some(rel) = entry.enclosed_name() else {
                bail!("{} contains an unsafe path", path.display());
            };
            let target = into.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&target)?;
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&target)?;
            std::io::copy(&mut entry, &mut out)?;

            // A shipped binary arrives without its mode, and a controller cannot exec what it
            // cannot run. Carried from the archive when it has one.
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode));
            }
        }

        // Swap it in. `rename` onto an existing directory fails, so the old tree moves aside
        // first and is deleted after — that way a failure here leaves the old one recoverable
        // rather than leaving nothing at all.
        let dest = staging.with_file_name(
            staging
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .rsplit_once(".unpack-")
                .map(|(name, _)| name.to_string())
                .unwrap_or_default(),
        );
        let previous = dest.with_extension(format!("old-{}-{}", std::process::id(), ticket));

        // Moving the old tree aside and the new one in has to happen as one step, or two
        // installers interleave: both see no destination, both skip the move-aside, and the
        // second rename lands on a directory the first just created. Within a process a lock
        // settles it; across processes the loser simply tries again, because by then the
        // destination exists and will be moved aside on the next pass.
        static SWAP: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let mut last: Option<std::io::Error> = None;
        for _ in 0..8 {
            let _held = SWAP.lock().unwrap_or_else(|e| e.into_inner());
            let had_previous = dest.exists();
            if had_previous && std::fs::rename(&dest, &previous).is_err() {
                continue; // somebody else is mid-swap; look again
            }
            match std::fs::rename(&staging, &dest) {
                Ok(()) => {
                    if had_previous {
                        let _ = std::fs::remove_dir_all(&previous);
                    }
                    return Ok(());
                }
                Err(e) => {
                    // Put the old tree back rather than leaving the driver with nothing.
                    if had_previous {
                        let _ = std::fs::rename(&previous, &dest);
                    }
                    last = Some(e);
                }
            }
        }
        let _ = std::fs::remove_dir_all(&staging);
        Err(last
            .map(anyhow::Error::from)
            .unwrap_or_else(|| anyhow::anyhow!("could not install {}", dest.display())))
    }

    /// Write a `.junodrv` from a directory containing `manifest.toml` and a payload.
    pub fn build(dir: &Path, out: &Path) -> Result<PathBuf> {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        // Same two layouts `read` accepts: one driver in `manifest.toml`, or several in
        // `manifests/`. Collected here in archive-name order so the package is byte-stable
        // between builds.
        let mut sources: Vec<(String, String)> = Vec::new();
        let root = dir.join("manifest.toml");
        if root.is_file() {
            sources.push(("manifest.toml".into(), std::fs::read_to_string(&root)?));
        }
        let extra_dir = dir.join("manifests");
        if extra_dir.is_dir() {
            let mut paths: Vec<_> = std::fs::read_dir(&extra_dir)?
                .filter_map(std::result::Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .collect();
            paths.sort();
            for p in paths {
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                sources.push((format!("manifests/{name}"), std::fs::read_to_string(&p)?));
            }
        }
        if sources.is_empty() {
            bail!(
                "{} declares no drivers: expected manifest.toml or manifests/*.toml",
                dir.display()
            );
        }

        let manifests = sources
            .iter()
            .map(|(name, src)| {
                Manifest::parse(src).with_context(|| format!("{}/{name}", dir.display()))
            })
            .collect::<Result<Vec<_>>>()?;
        let lead = if sources[0].0 == "manifest.toml" {
            0
        } else {
            lead_index(&manifests)
        };
        let manifest = &manifests[lead];

        std::fs::create_dir_all(out)?;
        let path = out.join(format!(
            "{}-{}.junodrv",
            manifest.driver.id, manifest.driver.version
        ));
        let file = std::fs::File::create(&path)?;
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default();

        for (name, src) in &sources {
            zip.start_file(name, opts)?;
            zip.write_all(src.as_bytes())?;
        }

        // An adapter has no payload file — its package *is* a tree, and the `exec` in its
        // `[adapter]` table is resolved against it. So everything beside the manifest goes in,
        // which is the only case where core cannot know in advance what the files are called.
        let is_adapter = sources.iter().any(|(_, src)| {
            toml::from_str::<Manifest>(src)
                .map(|m| m.driver.runtime == RuntimeKind::Adapter)
                .unwrap_or(false)
        });
        if is_adapter {
            let mut stack = vec![dir.to_path_buf()];
            while let Some(current) = stack.pop() {
                for entry in std::fs::read_dir(&current)?.filter_map(std::result::Result::ok) {
                    let p = entry.path();
                    // The archive being written, when it is being written into the directory
                    // it is packaging.
                    if p == path {
                        continue;
                    }
                    let Ok(rel) = p.strip_prefix(dir) else { continue };
                    let name = rel.to_string_lossy().replace('\\', "/");
                    // Already written above, and writing it twice makes an archive with two
                    // entries of the same name.
                    if name == "manifest.toml" || name.starts_with("manifests/") {
                        continue;
                    }
                    // An adapter's tree is taken whole, which means packing one *in place*
                    // takes the build directory with it — a debug binary, the rustc cache,
                    // every intermediate object. The result installs and even runs, so
                    // nothing complains; it is just a driver package hundreds of times larger
                    // than the driver. None of these is ever payload.
                    if matches!(
                        entry.file_name().to_string_lossy().as_ref(),
                        "target" | "node_modules" | ".git"
                    ) {
                        continue;
                    }
                    if p.is_dir() {
                        stack.push(p);
                        continue;
                    }
                    let mut opts = opts;
                    // A shipped binary that arrives without its exec bit is a package that
                    // installs and then cannot start.
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Ok(meta) = std::fs::metadata(&p) {
                            opts = opts.unix_permissions(meta.permissions().mode());
                        }
                    }
                    zip.start_file(&name, opts)?;
                    zip.write_all(&std::fs::read(&p)?)?;
                }
            }
            zip.finish()?;
            return Ok(path);
        }

        let mut wrote_payload = false;
        for candidate in ["commands.toml", "driver.wasm", "driver.py"] {
            let p = dir.join(candidate);
            if p.exists() {
                zip.start_file(candidate, opts)?;
                zip.write_all(&std::fs::read(&p)?)?;
                wrote_payload = true;
            }
        }
        // Every compiled plugin staged beside the manifest — one per platform the driver was
        // built for, each naming its own.
        for entry in std::fs::read_dir(dir)?.filter_map(std::result::Result::ok) {
            let name = entry.file_name().to_string_lossy().to_string();
            if is_payload(&name) {
                zip.start_file(&name, opts)?;
                zip.write_all(&std::fs::read(entry.path())?)?;
                wrote_payload = true;
            }
        }
        if !wrote_payload {
            bail!("{} has no payload to package", dir.display());
        }

        let readme = dir.join("README.md");
        if readme.exists() {
            zip.start_file("docs/README.md", opts)?;
            zip.write_all(&std::fs::read(readme)?)?;
        }

        // The driver's own settings screen, if it has one. One self-contained page: the
        // configurator renders it in a frame from the text, so a second file it tried to
        // fetch would not be there to fetch.
        let ui = dir.join("ui").join("index.html");
        if ui.exists() {
            zip.start_file("ui/index.html", opts)?;
            zip.write_all(&std::fs::read(ui)?)?;
        }

        zip.finish()?;
        Ok(path)
    }
}

/// A driver core knows about, whether or not any device uses it. This is the "available
/// drivers" list the configurator picks from when adding a device.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Available {
    pub driver_id: String,
    pub name: String,
    pub manufacturer: String,
    pub version: String,
    pub runtime: String,
    /// Device classes this driver provides.
    pub proxies: Vec<String>,
    /// `built-in`, `package`, or `sideloaded`.
    pub origin: String,
    /// Driver id of the bridge this driver's devices live behind, if any.
    pub parent: Option<String>,
    /// How many devices currently use it.
    pub devices: usize,
    pub readme: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, primary: bool, parent: Option<&str>) -> Manifest {
        let mut src = format!(
            "[driver]\nid = \"{id}\"\nname = \"x\"\nversion = \"1.0.0\"\n\
             runtime = \"native\"\nprimary = {primary}\n"
        );
        if let Some(p) = parent {
            src.push_str(&format!("parent = \"{p}\"\n"));
        }
        src.push_str("\n[[proxy]]\nid = 1\ntype = \"light\"\n");
        Manifest::parse(&src).expect("fixture parses")
    }

    /// Which driver leads decides the artifact's name and what the installer reports back, so
    /// a package that silently led with a remote sensor instead of the thermostat it belongs
    /// to was wrong in a way nobody would think to check.
    #[test]
    fn the_lead_is_declared_then_inferred_then_stable() {
        // Siblings with nothing to distinguish them: the package says which.
        let siblings = [
            manifest("roku.player", false, None),
            manifest("roku.tv", true, None),
        ];
        assert_eq!(lead_index(&siblings), 1);

        // A bridge leads its children without anyone writing it down, because `parent`
        // already says so — and it does that even when it sorts second.
        let bridged = [
            manifest("signify.hue.bulb", false, Some("signify.hue.bridge")),
            manifest("signify.hue.bridge", false, None),
        ];
        assert_eq!(lead_index(&bridged), 1);

        // An explicit primary outranks the parent relationship rather than tying with it.
        let both = [
            manifest("a.child", true, Some("a.bridge")),
            manifest("a.bridge", false, None),
        ];
        assert_eq!(lead_index(&both), 0);

        // Nothing to go on: first, which is arbitrary but must not vary between builds.
        let flat = [
            manifest("z.one", false, None),
            manifest("z.two", false, None),
        ];
        assert_eq!(lead_index(&flat), 0);
    }
}

#[cfg(test)]
mod unpack_tests {
    use super::*;

    /// A package with enough files that a torn write is easy to hit.
    fn a_package(dir: &Path, files: usize) -> PathBuf {
        std::fs::create_dir_all(dir.join("manifests")).unwrap();
        std::fs::write(
            dir.join("manifests/test.toml"),
            "[driver]\nid = \"t.t\"\nname = \"T\"\nmanufacturer = \"T\"\nversion = \"1.0.0\"\nruntime = \"adapter\"\n\n[adapter]\nexec = \"node\"\n\n[[proxy]]\nid = 1\ntype = \"bridge\"\nprimary = true\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        for i in 0..files {
            std::fs::write(dir.join("lib").join(format!("f{i}.js")), "x".repeat(200)).unwrap();
        }
        let out = dir.parent().unwrap().join("out");
        std::fs::create_dir_all(&out).unwrap();
        Package::build(dir, &out).expect("packs")
    }

    /// Unpacking the same package twice at once used to have one process delete the tree the
    /// other was still writing into — `Directory not empty`, or an adapter whose `exec` is
    /// briefly not on disk. Both must succeed and the result must be whole.
    #[test]
    fn two_unpacks_of_one_package_do_not_tear_each_other_down() {
        let tmp = std::env::temp_dir().join(format!("sdk-unpack-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        let pkg = a_package(&src, 300);
        let dest = tmp.join("installed");

        std::thread::scope(|s| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let pkg = pkg.clone();
                    let dest = dest.clone();
                    s.spawn(move || Package::unpack(&pkg, &dest))
                })
                .collect();
            for h in handles {
                h.join().unwrap().expect("every concurrent unpack succeeds");
            }
        });

        // Whole, not half: every file the archive carried is present.
        assert!(dest.join("manifests/test.toml").is_file());
        let n = std::fs::read_dir(dest.join("lib")).unwrap().count();
        assert_eq!(n, 300, "the tree is complete, not partially written");

        // And nothing is left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&tmp)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".unpack-") || n.contains(".old-"))
            .collect();
        assert!(leftovers.is_empty(), "staging dirs left behind: {leftovers:?}");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
