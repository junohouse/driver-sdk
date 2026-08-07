//! Loading `proxies/*.toml` into memory.

use super::schema::Proxy;

/// The contracts `build.rs` baked in.
mod generated {
    include!(concat!(env!("OUT_DIR"), "/proxies.rs"));
}

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct ProxyRegistry {
    proxies: BTreeMap<String, Proxy>,
}

impl ProxyRegistry {
    /// Load every `*.toml` in `dir`. Fails loudly on a bad contract — a malformed proxy is a
    /// build error, not something to limp along with.
    pub fn load_dir(dir: &Path) -> anyhow::Result<Self> {
        let mut proxies = BTreeMap::new();
        let mut problems = Vec::new();

        let mut files: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", dir.display()))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();

        for path in files {
            let src = std::fs::read_to_string(&path)?;
            let proxy = match Proxy::parse(&src) {
                Ok(p) => p,
                Err(e) => {
                    problems.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            if proxy.name != stem {
                problems.push(format!(
                    "{}: name `{}` does not match filename",
                    path.display(),
                    proxy.name
                ));
            }
            for e in proxy.validate() {
                problems.push(format!("{}: {e}", path.display()));
            }
            proxies.insert(proxy.name.clone(), proxy);
        }

        if !problems.is_empty() {
            anyhow::bail!("invalid proxy definitions:\n  {}", problems.join("\n  "));
        }
        Ok(ProxyRegistry { proxies })
    }

    /// The contracts shipped with this crate, compiled in.
    ///
    /// `JUNO_PROXIES` still overrides the location, which is what makes editing a contract and
    /// re-running without a rebuild possible. Everything else reads what `build.rs` baked in —
    /// so a binary carries its own contracts and does not depend on a directory existing on
    /// the machine that runs it.
    pub fn bundled() -> anyhow::Result<Self> {
        if let Ok(dir) = std::env::var("JUNO_PROXIES") {
            return ProxyRegistry::load_dir(Path::new(&dir));
        }
        let mut proxies = BTreeMap::new();
        let mut problems = Vec::new();
        for (name, src) in generated::PROXIES {
            let proxy = match Proxy::parse(src) {
                Ok(p) => p,
                Err(e) => {
                    problems.push(format!("{name}: {e}"));
                    continue;
                }
            };
            if proxy.name != *name {
                problems.push(format!("{name}: name `{}` does not match filename", proxy.name));
            }
            for e in proxy.validate() {
                problems.push(format!("{name}: {e}"));
            }
            proxies.insert(proxy.name.clone(), proxy);
        }
        if !problems.is_empty() {
            anyhow::bail!("invalid proxy definitions:\n  {}", problems.join("\n  "));
        }
        Ok(ProxyRegistry { proxies })
    }

    /// Which contracts are compiled in, by name. For a caller that wants the list without
    /// parsing them.
    pub fn bundled_names() -> impl Iterator<Item = &'static str> {
        generated::PROXIES.iter().map(|(name, _)| *name)
    }

    pub fn get(&self, name: &str) -> Option<&Proxy> {
        self.proxies.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Proxy> {
        self.proxies.values()
    }

    pub fn len(&self) -> usize {
        self.proxies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.proxies.is_empty()
    }
}
