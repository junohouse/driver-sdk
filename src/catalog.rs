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
    /// The connections this driver declares, by kind — `["mqtt"]`, `["ir_out"]`,
    /// `["tcp", "mqtt"]`.
    ///
    /// In the index because a catalog has to show a product it has never installed, and
    /// "reached two ways" is a thing about the product rather than about the copy on disk. A
    /// driver with more than one is a choice somebody makes while adding it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<String>,
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
    /// What an unknown node on a mesh has to look like — see
    /// [`crate::manifest::ZigbeeMatch`]. Here for the same reason as the four above it: a
    /// coordinator reporting a node nobody has a driver for should be answerable from the
    /// catalog, not only from what is already installed.
    #[serde(default)]
    pub zigbee: Vec<crate::manifest::ZigbeeMatch>,
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
    /// What changed in this release, as Markdown — the section `junodrv entry --changelog`
    /// lifted out of the driver's own CHANGELOG for this version.
    ///
    /// In the index rather than in the package because it has to be readable *before* the
    /// decision to install: an update that needs a person to say yes has to be able to say
    /// why, and the artifact answering that question is the one not downloaded yet.
    ///
    /// Always empty on a prerelease. A beta is a rolling build of `main` with a run number on
    /// it — there is no released version for a changelog section to be about, and asking every
    /// push to write release notes gets notes nobody wrote.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
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

    /// The newest release published, whether or not this core can run it.
    ///
    /// Separate from [`Entry::best_for`], which is what to *install*. This is what to *say*: a
    /// controller too old for the newest build has to be told that is why it is not being
    /// offered one, and `best_for` on its own cannot tell "nothing newer exists" from "the
    /// newer one is out of reach".
    pub fn newest(&self) -> Option<&Release> {
        self.versions
            .iter()
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

/// What a controller should do about a release that is not the one it is running.
///
/// The whole update policy, in one place, because it is one decision made in three: the pane
/// that badges a driver, the check that runs at start, and the button somebody presses all
/// have to agree about whether a build may be taken without asking. Two of them agreeing and
/// the third not is an update that installs itself under a house that was never told.
///
/// The rule is semver, which is the one thing a version number already means. Same major:
/// the new build is a drop-in for the old one, and a controller takes it on its own. New
/// major: the author is saying something does not carry over — a setting that moved, a
/// pairing that has to be done again, a device class that changed — so it waits for a person
/// and shows them [`Release::notes`] first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Update {
    /// Running what the catalog offers, or unable to tell — which is not the same thing, but
    /// calls for the same nothing.
    Current,
    /// Take it. Same major, so nothing a house has configured stops meaning what it meant.
    Automatic { version: String },
    /// Newer, and waiting to be asked for.
    Manual { version: String, why: String },
    /// Newer, and this controller cannot run it. Said out loud rather than hidden, so a driver
    /// that is stuck says why it is stuck.
    Blocked { version: String, needs_core: String },
}

impl Update {
    /// Whether a controller may install this without being told to.
    pub fn is_automatic(&self) -> bool {
        matches!(self, Update::Automatic { .. })
    }

    /// Whether there is anything newer at all — automatic or not.
    pub fn is_offered(&self) -> bool {
        !matches!(self, Update::Current)
    }

    /// The version on offer, if there is one.
    pub fn version(&self) -> Option<&str> {
        match self {
            Update::Current => None,
            Update::Automatic { version }
            | Update::Manual { version, .. }
            | Update::Blocked { version, .. } => Some(version),
        }
    }
}

impl Release {
    /// What to do about this release, given what is installed.
    ///
    /// `installed_commit` is what answers the question on a prerelease. A beta is a rolling
    /// build — every push to `main` is `1.2.0-beta.N` of the same unreleased `1.2.0`, and the
    /// run number is not a promise about anything — so on either side being a prerelease the
    /// comparison is the build stamp, and any difference is taken automatically. That is what
    /// beta *is*: a house that asked for the newest build, continuously.
    ///
    /// A build with no commit and no readable version is a sideload or something assembled by
    /// hand. Unknown is not stale, and offering an update against a build nothing can identify
    /// would be telling somebody to fix something nobody can see.
    pub fn update_from(
        &self,
        installed_version: &str,
        installed_commit: &str,
        core: &semver::Version,
    ) -> Update {
        // Every "there is something newer" answer goes through here, so a release this core
        // cannot run cannot be reported as an update by one path and blocked by another.
        fn newer(r: &Release, core: &semver::Version, u: impl FnOnce(String) -> Update) -> Update {
            if r.runs_on(core) {
                u(r.version.clone())
            } else {
                Update::Blocked {
                    version: r.version.clone(),
                    needs_core: r.core_req.clone(),
                }
            }
        }

        let (Ok(have), Some(want)) = (semver::Version::parse(installed_version), self.semver())
        else {
            // Nothing to compare but the stamp.
            return match (installed_commit.trim(), self.commit.trim()) {
                ("", _) | (_, "") => Update::Current,
                (a, b) if a == b => Update::Current,
                _ => newer(self, core, |version| Update::Automatic { version }),
            };
        };

        // Rolling. Neither side's number moves between builds, so it cannot be the answer.
        if !have.pre.is_empty() || !want.pre.is_empty() {
            return match (installed_commit.trim(), self.commit.trim()) {
                ("", _) | (_, "") => Update::Current,
                (a, b) if a == b => Update::Current,
                _ => newer(self, core, |version| Update::Automatic { version }),
            };
        }

        if want <= have {
            return Update::Current;
        }

        // Before 1.0 the minor carries what the major carries after it — the convention every
        // package manager already reads this way, and the one a driver at 0.4 is relying on.
        let breaking = if have.major == 0 || want.major == 0 {
            want.major != have.major || want.minor != have.minor
        } else {
            want.major != have.major
        };

        if breaking {
            newer(self, core, |version: String| Update::Manual {
                why: format!(
                    "{version} is a new major version of a driver installed at {have} — \
                     settings, pairings or devices may not carry over"
                ),
                version,
            })
        } else {
            newer(self, core, |version| Update::Automatic { version })
        }
    }
}

/// The section of a CHANGELOG that is about one version, as Markdown.
///
/// Keep-a-Changelog shape, loosely: a heading that names the version, everything under it, up
/// to the next heading at the same level or above. Loosely because a driver author writes this
/// file for people, and refusing to read `## [1.2.0] — 2026-08-01` because of the brackets or
/// the dash would mean the notes silently going missing from the one screen they exist for.
///
/// Empty for a prerelease, on purpose and not by accident of the file: see [`Release::notes`].
pub fn notes_for(changelog: &str, version: &str) -> String {
    let version = version.trim();
    if version.is_empty() || semver::Version::parse(version).is_ok_and(|v| !v.pre.is_empty()) {
        return String::new();
    }
    let level = |line: &str| line.chars().take_while(|c| *c == '#').count();
    let names = |line: &str| {
        line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
            .any(|word| word == version || word.strip_prefix('v') == Some(version))
    };

    let mut depth = None;
    let mut out: Vec<&str> = Vec::new();
    for line in changelog.lines() {
        match depth {
            None => {
                if level(line) > 0 && names(line) {
                    depth = Some(level(line));
                }
            }
            Some(d) => {
                if level(line) > 0 && level(line) <= d {
                    break;
                }
                out.push(line);
            }
        }
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    while out.first().is_some_and(|l| l.trim().is_empty()) {
        out.remove(0);
    }
    out.join("\n")
}

#[cfg(test)]
mod update_tests {
    use super::*;

    fn core() -> semver::Version {
        semver::Version::parse("0.9.0").unwrap()
    }

    fn release(version: &str, commit: &str, core_req: &str) -> Release {
        Release {
            version: version.into(),
            core_req: core_req.into(),
            url: String::new(),
            sha256: String::new(),
            size: 0,
            commit: commit.into(),
            notes: String::new(),
        }
    }

    #[test]
    fn a_patch_or_a_minor_is_taken_without_asking() {
        assert!(
            release("1.2.1", "", "")
                .update_from("1.2.0", "", &core())
                .is_automatic()
        );
        assert!(
            release("1.3.0", "", "")
                .update_from("1.2.0", "", &core())
                .is_automatic()
        );
    }

    #[test]
    fn a_new_major_waits_to_be_asked_for() {
        let update = release("2.0.0", "", "").update_from("1.2.0", "", &core());
        assert!(!update.is_automatic() && update.is_offered());
        assert!(matches!(update, Update::Manual { .. }));
    }

    /// Before 1.0 the minor is where a break lands, which is what every other package manager
    /// reads it as — and what a driver still at 0.x is relying on.
    #[test]
    fn before_one_point_oh_the_minor_is_the_break() {
        assert!(matches!(
            release("0.5.0", "", "").update_from("0.4.2", "", &core()),
            Update::Manual { .. }
        ));
        assert!(
            release("0.4.3", "", "")
                .update_from("0.4.2", "", &core())
                .is_automatic()
        );
    }

    /// Beta is a rolling build of one unreleased version: the number says nothing and the
    /// commit says everything.
    #[test]
    fn a_beta_follows_the_commit() {
        let build = release("1.0.0-beta.41", "bbbb", "");
        assert!(
            build
                .update_from("1.0.0-beta.40", "aaaa", &core())
                .is_automatic()
        );
        assert!(
            !build
                .update_from("1.0.0-beta.40", "bbbb", &core())
                .is_offered()
        );
        // Not every beta build even moves the run number, and it would not matter if it did.
        assert!(
            build
                .update_from("1.0.0-beta.41", "aaaa", &core())
                .is_automatic()
        );
    }

    #[test]
    fn nothing_is_said_about_a_build_nothing_can_identify() {
        // A sideload: no commit on the installed side, and a version that means nothing here.
        assert!(
            !release("1.0.0-beta.41", "bbbb", "")
                .update_from("1.0.0-beta.40", "", &core())
                .is_offered()
        );
        assert!(
            !release("1.2.0", "", "")
                .update_from("1.2.0", "", &core())
                .is_offered()
        );
        assert!(
            !release("1.1.0", "", "")
                .update_from("1.2.0", "", &core())
                .is_offered()
        );
    }

    /// A release this core cannot run is still shown — hiding it makes a stuck driver look
    /// current.
    #[test]
    fn a_release_needing_a_newer_core_is_named_not_hidden() {
        assert_eq!(
            release("2.0.0", "", ">=1.0").update_from("1.2.0", "", &core()),
            Update::Blocked {
                version: "2.0.0".into(),
                needs_core: ">=1.0".into()
            }
        );
        assert_eq!(
            release("1.2.1", "", ">=1.0").update_from("1.2.0", "", &core()),
            Update::Blocked {
                version: "1.2.1".into(),
                needs_core: ">=1.0".into()
            }
        );
    }

    const CHANGELOG: &str = "\
# Changelog

## [1.2.0] — 2026-08-01

### Added
- Input switching over the CNAME.

## 1.1.0

- The first one.
";

    #[test]
    fn the_notes_are_the_section_about_that_version() {
        assert_eq!(
            notes_for(CHANGELOG, "1.2.0"),
            "### Added\n- Input switching over the CNAME."
        );
        assert_eq!(notes_for(CHANGELOG, "1.1.0"), "- The first one.");
        assert_eq!(notes_for(CHANGELOG, "9.9.9"), "");
        // A beta has no released version for a section to be about.
        assert_eq!(notes_for(CHANGELOG, "1.2.0-beta.4"), "");
    }
}
