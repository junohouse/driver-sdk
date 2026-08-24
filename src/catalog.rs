//! The registry index format.
//!
//! What a catalog of drivers looks like on the wire: which drivers exist, where their builds
//! are, and which core versions each build can run on. Controllers fetch it, the public
//! catalog site renders it, and a driver's CI emits a row for the package it just built.
//!
//! It lives here for that last reason. Emitting an index row is part of publishing a driver,
//! and a driver's CI should not need read access to the controller's repository to describe
//! the artifact it has in its hand.
//!
//! What is *not* here is fetching or downloading. Deciding to reach out to the network, and
//! verifying what comes back, is a controller's job — see `junod`.

use crate::sddp::SddpMatch;
use serde::{Deserialize, Serialize};

/// Bumped when the index format changes incompatibly. Core refuses an index it cannot read
/// rather than silently showing a partial catalog.
pub const SUPPORTED_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    pub schema: u32,
    #[serde(default)]
    pub generated: String,
    #[serde(default)]
    pub drivers: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub manufacturer: String,
    /// Driver id of the bridge these devices live behind, straight from the manifest's
    /// [`crate::driver::manifest::DriverMeta::parent`].
    ///
    /// The runtime has always known this — a child reads its parent's properties rather than
    /// carrying its own copy of the bridge address. It is in the index so the catalog can show
    /// a product the way it is actually installed, without a reader guessing the hierarchy
    /// back out of id prefixes and getting it wrong.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// What a catalog should call the product this driver leads, when that is not its own name
    /// — straight from [`crate::manifest::DriverMeta::product`].
    ///
    /// In the index because a catalog lists products, and half of what it lists is not
    /// installed. Without it the shelf reads `Philips Hue Bridge` and `TP-Link Account` until
    /// somebody adds them, and something else afterwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    /// What that product *is*, as a device class — from [`crate::manifest::DriverMeta::kind`],
    /// falling back to the proxy the driver leads with.
    ///
    /// This is what groups the catalog and picks the icon, so it has to be answerable for a
    /// driver nobody has installed. Inferring it from `proxies` at the far end gets every hub
    /// wrong: a Hue package leads with a bridge, and a bridge is not what anybody bought.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The same product as the driver named here, reached another way — from
    /// [`crate::manifest::DriverMeta::variant_of`].
    ///
    /// In the index because the two halves of one product are two rows here, with no package to
    /// read the relationship out of: `apple.tv` and `apple.tv.ir` are siblings in one archive
    /// and strangers in a catalog until this says otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_of: Option<String>,
    /// How this driver reaches its hardware — from [`crate::manifest::Manifest::reach`].
    ///
    /// In the index because it is what tells one variant from another, and a catalog has to
    /// draw that distinction for products nobody has installed: two variants reached
    /// differently are a question somebody has to answer, and two reached the same way are one
    /// row whose own setup flow settles which it is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reach: Vec<String>,
    /// `junohouse/sony-bravia-ip`. The provenance claim.
    #[serde(default)]
    pub repo: String,
    /// Whether [`Entry::repo`] can actually be read.
    ///
    /// Certification is a claim about where an artifact came from, and it holds whether or not
    /// the source is public — but the catalog has been offering a "Source" link on every row,
    /// which is a 404 the moment a driver ships from a private repo. Stated in the index so
    /// the page knows without probing GitHub, and so a closed driver says so plainly rather
    /// than by a link that fails.
    ///
    /// Absent means unstated, which reads as open: every row written before this field existed
    /// came from a public repo. CI fills it in from the repo's actual visibility, so it cannot
    /// drift from the truth the way a hand-set flag would.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(default)]
    pub proxies: Vec<String>,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub description: String,
    /// Carried in the *index* so core can match a device found on the network against
    /// drivers that are not installed yet.
    #[serde(default)]
    pub discovery: DiscoveryHints,
    #[serde(default)]
    pub versions: Vec<Release>,
}

/// Whether a driver's source can be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Open,
    Closed,
}

impl Source {
    /// From a repository's visibility, which is the only thing that decides this.
    pub fn from_private(private: bool) -> Self {
        if private {
            Source::Closed
        } else {
            Source::Open
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiscoveryHints {
    /// A bare service type, or a service type plus what its TXT must say — see
    /// [`crate::mdns::MdnsMatch`]. Bare strings are the shorthand, so rows published before
    /// this existed read unchanged.
    #[serde(default)]
    pub mdns: Vec<crate::mdns::MdnsMatch>,
    #[serde(default)]
    pub ssdp: Vec<String>,
    /// What an SDDP announcement has to look like. See [`SddpMatch`].
    #[serde(default)]
    pub sddp: Vec<SddpMatch>,
    /// First three octets of the MAC, any separator style.
    #[serde(default)]
    pub mac_oui: Vec<String>,
    /// What to broadcast, and what a reply of this driver's looks like. See
    /// [`crate::udp::UdpMatch`].
    ///
    /// In the index, not just the manifest, because this is a *listening* discovery like the
    /// three above it and not a sweep like `[[transport]] probe`: it runs against the whole
    /// catalog so a controller with nothing installed still finds the hardware. Core needs the
    /// port and the payload from here to know what to send.
    #[serde(default)]
    pub udp: Vec<crate::udp::UdpMatch>,
    /// The driver a find under these rules should actually be adopted as, once this one is set
    /// up — see [`crate::manifest::Discovery::adopt_as`]. In the index because the first half
    /// of that two-stage answer has to work before anything is installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopt_as: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    /// Which core versions can run this build, as a semver requirement (`">=0.4"`).
    #[serde(rename = "core", default)]
    pub core_req: String,
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    /// The driver repository's commit this was built from.
    ///
    /// What "which build is this" means while nothing is released. Every driver here tracks
    /// `main` and cuts no tags — see the note in core's CLAUDE.md about what tagging cost —
    /// so the `version` in a manifest stays whatever somebody last typed and says nothing
    /// about whether an installed copy is current. The commit does, exactly, and it is the
    /// only thing that does.
    ///
    /// Empty when the build came from somewhere without one: a sideload, or a package built
    /// by hand. Then "is this current" is unanswerable, which is the honest answer.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit: String,
}

/// What a discovery probe saw on the network.
#[derive(Debug, Clone, Default)]
pub struct Discovered {
    pub mdns: Vec<String>,
    /// The TXT of the advertisement in `mdns`, flattened. Carried because a service type is
    /// not always a claim: `_airplay._tcp` is every AirPlay receiver on the network, and the
    /// thing that separates them is what they say about themselves.
    pub mdns_txt: std::collections::BTreeMap<String, String>,
    pub ssdp: Vec<String>,
    /// Whole announcements, not just their type — matching happens across several of their
    /// fields at once, so the fields have to survive this far.
    pub sddp: Vec<crate::sddp::Found>,
    pub mac: Option<String>,
    /// Whole replies, for the same reason `sddp` keeps whole announcements: a vendor's format
    /// is the vendor's, and the port a reply came back on is half of what identifies it.
    pub udp: Vec<crate::udp::Found>,
}

/// Normalize a MAC or OUI to bare uppercase hex so `FC:F1:52`, `fc-f1-52` and `FCF152` all
/// compare equal. Vendors are not consistent and neither are driver authors.
///
/// Public because a controller matches the same rules against its *installed* manifests as
/// this matches against the catalog, and the two have to normalize identically — a second copy
/// of this is a second answer to "is `fc-f1-52` this device", arrived at somewhere else.
pub fn norm_mac(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

impl Index {
    pub fn parse(src: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(src)
    }

    /// Refuse an index from a newer format rather than showing a partial catalog.
    pub fn check_schema(&self) -> Result<(), String> {
        if self.schema > SUPPORTED_SCHEMA {
            return Err(format!(
                "registry index is schema {} but this core understands {SUPPORTED_SCHEMA} — \
                 update core",
                self.schema
            ));
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.drivers.iter().find(|d| d.id == id)
    }

    /// Drivers that could plausibly be for a device we just found on the network. This is
    /// what turns the Discovery list from a dead end into one click.
    pub fn match_discovery(&self, found: &Discovered) -> Vec<&Entry> {
        let mac = found.mac.as_deref().map(norm_mac);
        self.drivers
            .iter()
            .filter(|d| {
                let h = &d.discovery;
                let service_hit = |theirs: &[String], ours: &[String]| {
                    theirs.iter().any(|t| ours.iter().any(|o| o == t))
                };
                h.mdns
                    .iter()
                    .any(|rule| found.mdns.iter().any(|s| rule.matches(s, &found.mdns_txt)))
                    || service_hit(&h.ssdp, &found.ssdp)
                    || h.sddp
                        .iter()
                        .any(|rule| found.sddp.iter().any(|a| rule.matches(a)))
                    || mac.as_ref().is_some_and(|m| {
                        h.mac_oui
                            .iter()
                            .any(|o| m.starts_with(&norm_mac(o)) && !norm_mac(o).is_empty())
                    })
                    || h.udp
                        .iter()
                        .any(|rule| found.udp.iter().any(|reply| rule.matches(reply)))
            })
            .collect()
    }

    /// Every driver providing a given proxy — "what can I use for a thermostat?"
    pub fn providing(&self, proxy: &str) -> Vec<&Entry> {
        self.drivers
            .iter()
            .filter(|d| d.proxies.iter().any(|p| p == proxy))
            .collect()
    }
}

impl Entry {
    /// The newest release this core can actually run.
    ///
    /// Resolution lives here, not in the registry, on purpose: a registry that assumed every
    /// controller was current would strand exactly the installations least able to update.
    pub fn best_for(&self, core_version: &semver::Version) -> Option<&Release> {
        self.versions
            .iter()
            .filter(|r| r.runs_on(core_version))
            .filter_map(|r| r.semver().map(|v| (v, r)))
            .max_by(|(a, _), (b, _)| a.cmp(b))
            .map(|(_, r)| r)
    }

    /// Releases this core cannot run, newest first — so the UI can say *why* an update is not
    /// being offered instead of just hiding it.
    pub fn blocked_for(&self, core_version: &semver::Version) -> Vec<&Release> {
        let mut out: Vec<_> = self
            .versions
            .iter()
            .filter(|r| !r.runs_on(core_version))
            .collect();
        out.sort_by(|a, b| b.semver().cmp(&a.semver()));
        out
    }
}

impl Release {
    pub fn semver(&self) -> Option<semver::Version> {
        semver::Version::parse(&self.version).ok()
    }

    pub fn runs_on(&self, core_version: &semver::Version) -> bool {
        if self.core_req.trim().is_empty() {
            return true;
        }
        match semver::VersionReq::parse(&self.core_req) {
            Ok(req) => req.matches(core_version),
            // An unparseable requirement is the registry's bug, not the user's. Refusing to
            // install is safer than guessing it is compatible.
            Err(_) => false,
        }
    }
}
